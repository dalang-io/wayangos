/*
 * WayangOS POS Kiosk v3.1 - Full POS System
 * Direct Framebuffer + Input, SQLite backend
 * Amber/gold theme, double buffered
 * Keyboard + mouse + touchscreen via /dev/input + /dev/tty
 *
 * Build without SQLite: gcc -static -O2 -o pos fbpos-v3.c -lm
 * Build with SQLite:    gcc -static -O2 -o pos fbpos-v3.c sqlite3.c -lm -lpthread -DSQLITE_INTEGRATION
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <dirent.h>
#include <time.h>
#include <errno.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/select.h>
#include <sys/stat.h>
#include <linux/fb.h>
#include <linux/input.h>
#include <linux/kd.h>

#ifdef SQLITE_INTEGRATION
#include "sqlite3.h"
#endif

#include "font8x16.h"

/* ===== Framebuffer ===== */
typedef struct { unsigned char *mem; int w,h,bpp,stride; long size; } FB;

static inline void px(FB *f, int x, int y, int r, int g, int b) {
    if(x<0||x>=f->w||y<0||y>=f->h) return;
    long o=y*f->stride+x*f->bpp;
    f->mem[o]=b;f->mem[o+1]=g;f->mem[o+2]=r;if(f->bpp==4)f->mem[o+3]=0xFF;
}

static void rect(FB *f,int x,int y,int w,int h,int r,int g,int b) {
    if(x<0){w+=x;x=0;} if(y<0){h+=y;y=0;}
    if(w<=0||h<=0)return;
    for(int j=y;j<y+h&&j<f->h;j++) for(int i=x;i<x+w&&i<f->w;i++) px(f,i,j,r,g,b);
}

static void hline(FB *f,int x,int y,int w,int r,int g,int b) { rect(f,x,y,w,1,r,g,b); }
static void vline(FB *f,int x,int y,int h,int r,int g,int b) { rect(f,x,y,1,h,r,g,b); }

static void draw_char(FB *f,int x,int y,char c,int sc,int r,int g,int b) {
    if(c<32||c>126)c='?';
    int idx=c-32;
    for(int row=0;row<16;row++){
        unsigned char bits=font8x16[idx][row];
        for(int col=0;col<8;col++)
            if(bits&(0x80>>col)){
                if(sc==1) px(f,x+col,y+row,r,g,b);
                else rect(f,x+col*sc,y+row*sc,sc,sc,r,g,b);
            }
    }
}

static void text(FB *f,int x,int y,const char *s,int sc,int r,int g,int b) {
    while(*s){draw_char(f,x,y,*s,sc,r,g,b);x+=8*sc+sc;s++;}
}

static int tw(const char *s,int sc){int n=strlen(s);return n>0?n*(8*sc+sc)-sc:0;}

static void text_right(FB *f,int rx,int y,const char *s,int sc,int r,int g,int b) {
    text(f,rx-tw(s,sc),y,s,sc,r,g,b);
}

static void text_center(FB *f,int cx,int y,const char *s,int sc,int r,int g,int b) {
    text(f,cx-tw(s,sc)/2,y,s,sc,r,g,b);
}

/* ===== Amber/Gold Palette ===== */
enum {
    C_BG_R=20,C_BG_G=18,C_BG_B=12,
    C_PNL_R=30,C_PNL_G=27,C_PNL_B=18,
    C_BRD_R=70,C_BRD_G=60,C_BRD_B=25,
    C_GOLD_R=220,C_GOLD_G=180,C_GOLD_B=40,
    C_AMB_R=200,C_AMB_G=150,C_AMB_B=20,
    C_TXT_R=200,C_TXT_G=190,C_TXT_B=160,
    C_DIM_R=120,C_DIM_G=110,C_DIM_B=80,
    C_GRN_R=40,C_GRN_G=160,C_GRN_B=60,
    C_BLU_R=40,C_BLU_G=80,C_BLU_B=180,
    C_SEL_R=50,C_SEL_G=42,C_SEL_B=15,
    C_HI_R=60,C_HI_G=50,C_HI_B=20,      /* highlight for selected order line */
    C_RED_R=180,C_RED_G=40,C_RED_B=40,
};

/* ===== App State Machine ===== */
enum {
    STATE_WELCOME = 0,
    STATE_LOGIN,
    STATE_POS,
    STATE_MENU_MGMT,
    STATE_USER_MGMT,
    STATE_ORDER_HISTORY,
    STATE_PAID_CONFIRM,
};

static int app_state = STATE_WELCOME;

/* ===== Menu & Order ===== */
typedef struct {
    int id;          /* database id (0 if not in db yet) */
    char name[32];
    int price;
    int cat;         /* 0=drinks, 1=food, 2=snacks */
    int active;
} MenuItem;

typedef struct { int idx; int qty; } OrderLine;

/* Dynamic menu - loaded from DB or defaults */
static MenuItem menu_items[64];
static int menu_count = 0;

static const char *cats[]={"ALL","DRINKS","FOOD","SNACKS"};
static int cur_cat=0;
static OrderLine order[32];
static int ocnt=0, onum=1;
static int order_sel=0;  /* selected order line index for navigation */
static int order_scroll=0;  /* scroll offset for order list (touch scrolling) */

/* Payment method: 0=CASH, 1=QRIS */
static int pay_method = 0;

/* Pagination for menu grid */
static int menu_page = 0;

/* F1 Help overlay */
static int show_help = 0;

/* Current logged-in user */
static int current_user_id = 0;
static char current_username[32] = "";
static char current_role[16] = "";

/* Login screen state */
static char login_user[32] = "";
static char login_pass[32] = "";
static int login_field = 0;  /* 0=username, 1=password */
static char login_error[64] = "";

/* Menu management state */
static int mgmt_sel = 0;
static int mgmt_mode = 0;  /* 0=list, 1=add, 2=edit */
static char mgmt_name[32] = "";
static char mgmt_price[16] = "";
static int mgmt_cat = 0;
static int mgmt_field = 0;  /* 0=name, 1=price, 2=cat */
static int mgmt_edit_id = 0;

/* User management state */
static int umgmt_sel = 0;
static int umgmt_mode = 0;  /* 0=list, 1=add, 2=edit */
static char umgmt_uname[32] = "";
static char umgmt_pass[32] = "";
static int umgmt_role = 0;  /* 0=admin, 1=cashier */
static int umgmt_field = 0;
static int umgmt_edit_id = 0;

/* Order history state */
static int hist_sel = 0;
static int hist_detail = 0;  /* 0=list, 1=detail view */
static int hist_detail_id = 0;

/* Paid confirmation timer */
static time_t paid_time = 0;

/* User list for management */
typedef struct { int id; char username[32]; char role[16]; } UserEntry;
static UserEntry user_list[64];
static int user_count = 0;

/* Order history list */
typedef struct { int id; int total; int tax; char method[8]; char cashier[32]; char date[20]; } HistOrder;
static HistOrder hist_orders[256];
static int hist_count = 0;

/* Order detail items */
typedef struct { char name[32]; int qty; int price; } HistItem;
static HistItem hist_items[32];
static int hist_item_count = 0;

/* ===== Virtual Keyboard State ===== */
static int kb_visible = 0;       /* 0=hidden, 1=shown */
static int kb_shift = 0;         /* 0=lowercase, 1=uppercase */
static int kb_password_mode = 0; /* 0=normal, 1=show asterisks */
static char *kb_target_buf = NULL;   /* pointer to the buffer being edited */
static int kb_target_maxlen = 0;     /* max length of target buffer */
static int kb_target_field = -1;     /* which field triggered the keyboard */
/* kb_target_field codes:
   0=login_user, 1=login_pass,
   2=mgmt_name, 3=mgmt_price,
   4=umgmt_uname, 5=umgmt_pass */

static void kb_show(char *buf, int maxlen, int field, int password) {
    kb_visible = 1;
    kb_target_buf = buf;
    kb_target_maxlen = maxlen;
    kb_target_field = field;
    kb_password_mode = password;
    kb_shift = 0;
}

static void kb_hide(void) {
    kb_visible = 0;
    kb_target_buf = NULL;
    kb_target_field = -1;
}

static void kb_type_char(char c) {
    if(!kb_target_buf) return;
    int len = strlen(kb_target_buf);
    if(len < kb_target_maxlen - 1) {
        kb_target_buf[len] = c;
        kb_target_buf[len+1] = 0;
    }
}

static void kb_backspace(void) {
    if(!kb_target_buf) return;
    int len = strlen(kb_target_buf);
    if(len > 0) kb_target_buf[len-1] = 0;
}

/* ===== Default menu items ===== */
static void load_default_menu(void) {
    menu_count = 0;
    struct { const char *name; int price; int cat; } defaults[] = {
        {"Kopi Tubruk",    8000,  0},
        {"Es Kopi Susu",  15000,  0},
        {"Teh Tarik",     10000,  0},
        {"Nasi Goreng",   18000,  1},
        {"Mie Goreng",    15000,  1},
        {"Roti Bakar",    12000,  1},
        {"Pisang Goreng",  8000,  2},
        {"Es Jeruk",       8000,  2},
        {"Indomie",       10000,  2},
    };
    for(int i=0;i<9;i++){
        strncpy(menu_items[i].name, defaults[i].name, 31);
        menu_items[i].name[31]=0;
        menu_items[i].price = defaults[i].price;
        menu_items[i].cat = defaults[i].cat;
        menu_items[i].active = 1;
        menu_items[i].id = i+1;
        menu_count++;
    }
}

/* ===== Order operations ===== */
static void order_add(int menu_idx){
    if(menu_idx<0||menu_idx>=menu_count||!menu_items[menu_idx].active) return;
    for(int j=0;j<ocnt;j++) if(order[j].idx==menu_idx){order[j].qty++;return;}
    if(ocnt<32){order[ocnt].idx=menu_idx;order[ocnt].qty=1;ocnt++;order_sel=ocnt-1;}
}
static void order_dec_selected(void){
    if(ocnt<=0||order_sel<0||order_sel>=ocnt) return;
    order[order_sel].qty--;
    if(order[order_sel].qty<=0){
        for(int k=order_sel;k<ocnt-1;k++)order[k]=order[k+1];
        ocnt--;
        if(order_sel>=ocnt&&ocnt>0) order_sel=ocnt-1;
        if(ocnt==0) order_sel=0;
    }
}
static void order_remove_selected(void){
    if(ocnt<=0||order_sel<0||order_sel>=ocnt) return;
    for(int k=order_sel;k<ocnt-1;k++)order[k]=order[k+1];
    ocnt--;
    if(order_sel>=ocnt&&ocnt>0) order_sel=ocnt-1;
    if(ocnt==0) order_sel=0;
}
static void order_inc_at(int i){
    if(i>=0&&i<ocnt) order[i].qty++;
}
static void order_dec_at(int i){
    if(i<0||i>=ocnt) return;
    order[i].qty--;
    if(order[i].qty<=0){
        for(int k=i;k<ocnt-1;k++)order[k]=order[k+1];
        ocnt--;
        if(order_sel>=ocnt&&ocnt>0) order_sel=ocnt-1;
        if(ocnt==0) order_sel=0;
    }
}
static void order_clear(void){ocnt=0;order_sel=0;order_scroll=0;}
static int order_total(void){int t=0;for(int i=0;i<ocnt;i++)t+=order[i].qty*menu_items[order[i].idx].price;return t;}
static int order_count_items(void){int t=0;for(int i=0;i<ocnt;i++)t+=order[i].qty;return t;}

static void fmt_rp(char *buf, int sz, int val) {
    if(val==0){snprintf(buf,sz,"Rp 0");return;}
    snprintf(buf, sz, "Rp %d.%03d", val/1000, val%1000);
}

/* ===== Hit regions ===== */
typedef struct{int x,y,w,h,act;}Hit;
static Hit hits[256];
static int nhit=0;

static int hit_test(int mx,int my){
    /* Search backwards: last-added (topmost/overlay) hits take priority */
    for(int i=nhit-1;i>=0;i--)
        if(mx>=hits[i].x&&mx<hits[i].x+hits[i].w&&my>=hits[i].y&&my<hits[i].y+hits[i].h)
            return hits[i].act;
    return -1;
}

/* ===== SQLite Integration ===== */
#ifdef SQLITE_INTEGRATION
static sqlite3 *db = NULL;

static int db_init(void) {
    /* Create /data directory if it doesn't exist */
    mkdir("/data", 0755);

    int rc = sqlite3_open("/data/pos.db", &db);
    if(rc != SQLITE_OK) {
        fprintf(stderr, "Cannot open database: %s\n", sqlite3_errmsg(db));
        return -1;
    }

    /* Enable WAL mode for better concurrency */
    sqlite3_exec(db, "PRAGMA journal_mode=WAL;", NULL, NULL, NULL);

    /* Create tables */
    const char *sql =
        "CREATE TABLE IF NOT EXISTS users ("
        "  id INTEGER PRIMARY KEY AUTOINCREMENT,"
        "  username TEXT UNIQUE NOT NULL,"
        "  password_hash TEXT NOT NULL,"
        "  role TEXT NOT NULL DEFAULT 'cashier',"
        "  created_at TEXT DEFAULT (datetime('now','localtime'))"
        ");"
        "CREATE TABLE IF NOT EXISTS menu_items ("
        "  id INTEGER PRIMARY KEY AUTOINCREMENT,"
        "  name TEXT NOT NULL,"
        "  price INTEGER NOT NULL,"
        "  category INTEGER NOT NULL,"
        "  active INTEGER NOT NULL DEFAULT 1,"
        "  created_at TEXT DEFAULT (datetime('now','localtime'))"
        ");"
        "CREATE TABLE IF NOT EXISTS orders ("
        "  id INTEGER PRIMARY KEY AUTOINCREMENT,"
        "  user_id INTEGER,"
        "  total INTEGER,"
        "  tax INTEGER,"
        "  payment_method TEXT,"
        "  created_at TEXT DEFAULT (datetime('now','localtime'))"
        ");"
        "CREATE TABLE IF NOT EXISTS order_items ("
        "  id INTEGER PRIMARY KEY AUTOINCREMENT,"
        "  order_id INTEGER,"
        "  menu_item_id INTEGER,"
        "  qty INTEGER,"
        "  price INTEGER"
        ");";

    char *err = NULL;
    rc = sqlite3_exec(db, sql, NULL, NULL, &err);
    if(rc != SQLITE_OK) {
        fprintf(stderr, "SQL error: %s\n", err);
        sqlite3_free(err);
        return -1;
    }

    /* Check if we need to seed default data */
    sqlite3_stmt *stmt;
    int count = 0;

    /* Seed default admin user */
    sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM users", -1, &stmt, NULL);
    if(sqlite3_step(stmt) == SQLITE_ROW) count = sqlite3_column_int(stmt, 0);
    sqlite3_finalize(stmt);

    if(count == 0) {
        /* Simple password hash: just store plain for now (in production, use bcrypt etc.)
         * For this embedded POS, we'll use a simple XOR-based "hash" */
        sqlite3_exec(db,
            "INSERT INTO users (username, password_hash, role) VALUES ('admin', 'admin123', 'admin');"
            "INSERT INTO users (username, password_hash, role) VALUES ('kasir', 'kasir123', 'cashier');",
            NULL, NULL, NULL);
    }

    /* Seed default menu items */
    count = 0;
    sqlite3_prepare_v2(db, "SELECT COUNT(*) FROM menu_items", -1, &stmt, NULL);
    if(sqlite3_step(stmt) == SQLITE_ROW) count = sqlite3_column_int(stmt, 0);
    sqlite3_finalize(stmt);

    if(count == 0) {
        sqlite3_exec(db,
            "INSERT INTO menu_items (name, price, category) VALUES ('Kopi Tubruk', 8000, 0);"
            "INSERT INTO menu_items (name, price, category) VALUES ('Es Kopi Susu', 15000, 0);"
            "INSERT INTO menu_items (name, price, category) VALUES ('Teh Tarik', 10000, 0);"
            "INSERT INTO menu_items (name, price, category) VALUES ('Nasi Goreng', 18000, 1);"
            "INSERT INTO menu_items (name, price, category) VALUES ('Mie Goreng', 15000, 1);"
            "INSERT INTO menu_items (name, price, category) VALUES ('Roti Bakar', 12000, 1);"
            "INSERT INTO menu_items (name, price, category) VALUES ('Pisang Goreng', 8000, 2);"
            "INSERT INTO menu_items (name, price, category) VALUES ('Es Jeruk', 8000, 2);"
            "INSERT INTO menu_items (name, price, category) VALUES ('Indomie', 10000, 2);",
            NULL, NULL, NULL);
    }

    /* Get next order number */
    sqlite3_prepare_v2(db, "SELECT COALESCE(MAX(id),0)+1 FROM orders", -1, &stmt, NULL);
    if(sqlite3_step(stmt) == SQLITE_ROW) onum = sqlite3_column_int(stmt, 0);
    sqlite3_finalize(stmt);

    return 0;
}

static void db_load_menu(void) {
    menu_count = 0;
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "SELECT id, name, price, category, active FROM menu_items ORDER BY id", -1, &stmt, NULL);
    while(sqlite3_step(stmt) == SQLITE_ROW && menu_count < 64) {
        menu_items[menu_count].id = sqlite3_column_int(stmt, 0);
        strncpy(menu_items[menu_count].name, (const char*)sqlite3_column_text(stmt, 1), 31);
        menu_items[menu_count].name[31] = 0;
        menu_items[menu_count].price = sqlite3_column_int(stmt, 2);
        menu_items[menu_count].cat = sqlite3_column_int(stmt, 3);
        menu_items[menu_count].active = sqlite3_column_int(stmt, 4);
        menu_count++;
    }
    sqlite3_finalize(stmt);
}

static int db_authenticate(const char *user, const char *pass) {
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "SELECT id, username, role FROM users WHERE username=? AND password_hash=?", -1, &stmt, NULL);
    sqlite3_bind_text(stmt, 1, user, -1, SQLITE_STATIC);
    sqlite3_bind_text(stmt, 2, pass, -1, SQLITE_STATIC);
    int found = 0;
    if(sqlite3_step(stmt) == SQLITE_ROW) {
        current_user_id = sqlite3_column_int(stmt, 0);
        strncpy(current_username, (const char*)sqlite3_column_text(stmt, 1), 31);
        strncpy(current_role, (const char*)sqlite3_column_text(stmt, 2), 15);
        found = 1;
    }
    sqlite3_finalize(stmt);
    return found;
}

static int db_save_order(void) {
    if(ocnt <= 0) return -1;
    int total = order_total();
    int tax = total * 11 / 100;
    int grand = total + tax;
    const char *method = pay_method == 0 ? "CASH" : "QRIS";

    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "INSERT INTO orders (user_id, total, tax, payment_method) VALUES (?,?,?,?)", -1, &stmt, NULL);
    sqlite3_bind_int(stmt, 1, current_user_id);
    sqlite3_bind_int(stmt, 2, grand);
    sqlite3_bind_int(stmt, 3, tax);
    sqlite3_bind_text(stmt, 4, method, -1, SQLITE_STATIC);
    sqlite3_step(stmt);
    sqlite3_finalize(stmt);

    int order_id = (int)sqlite3_last_insert_rowid(db);

    for(int i = 0; i < ocnt; i++) {
        sqlite3_prepare_v2(db, "INSERT INTO order_items (order_id, menu_item_id, qty, price) VALUES (?,?,?,?)", -1, &stmt, NULL);
        sqlite3_bind_int(stmt, 1, order_id);
        sqlite3_bind_int(stmt, 2, menu_items[order[i].idx].id);
        sqlite3_bind_int(stmt, 3, order[i].qty);
        sqlite3_bind_int(stmt, 4, menu_items[order[i].idx].price);
        sqlite3_step(stmt);
        sqlite3_finalize(stmt);
    }

    onum = order_id + 1;
    return order_id;
}

static void db_load_users(void) {
    user_count = 0;
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "SELECT id, username, role FROM users ORDER BY id", -1, &stmt, NULL);
    while(sqlite3_step(stmt) == SQLITE_ROW && user_count < 64) {
        user_list[user_count].id = sqlite3_column_int(stmt, 0);
        strncpy(user_list[user_count].username, (const char*)sqlite3_column_text(stmt, 1), 31);
        strncpy(user_list[user_count].role, (const char*)sqlite3_column_text(stmt, 2), 15);
        user_count++;
    }
    sqlite3_finalize(stmt);
}

static void db_add_user(const char *uname, const char *pass, const char *role) {
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "INSERT INTO users (username, password_hash, role) VALUES (?,?,?)", -1, &stmt, NULL);
    sqlite3_bind_text(stmt, 1, uname, -1, SQLITE_STATIC);
    sqlite3_bind_text(stmt, 2, pass, -1, SQLITE_STATIC);
    sqlite3_bind_text(stmt, 3, role, -1, SQLITE_STATIC);
    sqlite3_step(stmt);
    sqlite3_finalize(stmt);
}

static void db_update_user(int id, const char *uname, const char *pass, const char *role) {
    sqlite3_stmt *stmt;
    if(pass[0]) {
        sqlite3_prepare_v2(db, "UPDATE users SET username=?, password_hash=?, role=? WHERE id=?", -1, &stmt, NULL);
        sqlite3_bind_text(stmt, 1, uname, -1, SQLITE_STATIC);
        sqlite3_bind_text(stmt, 2, pass, -1, SQLITE_STATIC);
        sqlite3_bind_text(stmt, 3, role, -1, SQLITE_STATIC);
        sqlite3_bind_int(stmt, 4, id);
    } else {
        sqlite3_prepare_v2(db, "UPDATE users SET username=?, role=? WHERE id=?", -1, &stmt, NULL);
        sqlite3_bind_text(stmt, 1, uname, -1, SQLITE_STATIC);
        sqlite3_bind_text(stmt, 2, role, -1, SQLITE_STATIC);
        sqlite3_bind_int(stmt, 3, id);
    }
    sqlite3_step(stmt);
    sqlite3_finalize(stmt);
}

static void db_delete_user(int id) {
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "DELETE FROM users WHERE id=?", -1, &stmt, NULL);
    sqlite3_bind_int(stmt, 1, id);
    sqlite3_step(stmt);
    sqlite3_finalize(stmt);
}

static void db_add_menu_item(const char *name, int price, int cat) {
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "INSERT INTO menu_items (name, price, category) VALUES (?,?,?)", -1, &stmt, NULL);
    sqlite3_bind_text(stmt, 1, name, -1, SQLITE_STATIC);
    sqlite3_bind_int(stmt, 2, price);
    sqlite3_bind_int(stmt, 3, cat);
    sqlite3_step(stmt);
    sqlite3_finalize(stmt);
}

static void db_update_menu_item(int id, const char *name, int price, int cat) {
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "UPDATE menu_items SET name=?, price=?, category=? WHERE id=?", -1, &stmt, NULL);
    sqlite3_bind_text(stmt, 1, name, -1, SQLITE_STATIC);
    sqlite3_bind_int(stmt, 2, price);
    sqlite3_bind_int(stmt, 3, cat);
    sqlite3_bind_int(stmt, 4, id);
    sqlite3_step(stmt);
    sqlite3_finalize(stmt);
}

static void db_toggle_menu_item(int id) {
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "UPDATE menu_items SET active = NOT active WHERE id=?", -1, &stmt, NULL);
    sqlite3_bind_int(stmt, 1, id);
    sqlite3_step(stmt);
    sqlite3_finalize(stmt);
}

static void db_delete_menu_item(int id) {
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db, "DELETE FROM menu_items WHERE id=?", -1, &stmt, NULL);
    sqlite3_bind_int(stmt, 1, id);
    sqlite3_step(stmt);
    sqlite3_finalize(stmt);
}

static void db_load_order_history(void) {
    hist_count = 0;
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db,
        "SELECT o.id, o.total, o.tax, o.payment_method, COALESCE(u.username,'?'), o.created_at "
        "FROM orders o LEFT JOIN users u ON o.user_id=u.id ORDER BY o.id DESC LIMIT 256",
        -1, &stmt, NULL);
    while(sqlite3_step(stmt) == SQLITE_ROW && hist_count < 256) {
        hist_orders[hist_count].id = sqlite3_column_int(stmt, 0);
        hist_orders[hist_count].total = sqlite3_column_int(stmt, 1);
        hist_orders[hist_count].tax = sqlite3_column_int(stmt, 2);
        const char *m = (const char*)sqlite3_column_text(stmt, 3);
        strncpy(hist_orders[hist_count].method, m?m:"?", 7);
        const char *c = (const char*)sqlite3_column_text(stmt, 4);
        strncpy(hist_orders[hist_count].cashier, c?c:"?", 31);
        const char *d = (const char*)sqlite3_column_text(stmt, 5);
        strncpy(hist_orders[hist_count].date, d?d:"?", 19);
        hist_count++;
    }
    sqlite3_finalize(stmt);
}

static void db_load_order_detail(int order_id) {
    hist_item_count = 0;
    sqlite3_stmt *stmt;
    sqlite3_prepare_v2(db,
        "SELECT COALESCE(m.name, 'Deleted Item'), oi.qty, oi.price "
        "FROM order_items oi LEFT JOIN menu_items m ON oi.menu_item_id=m.id "
        "WHERE oi.order_id=? ORDER BY oi.id",
        -1, &stmt, NULL);
    sqlite3_bind_int(stmt, 1, order_id);
    while(sqlite3_step(stmt) == SQLITE_ROW && hist_item_count < 32) {
        const char *n = (const char*)sqlite3_column_text(stmt, 0);
        strncpy(hist_items[hist_item_count].name, n?n:"?", 31);
        hist_items[hist_item_count].qty = sqlite3_column_int(stmt, 1);
        hist_items[hist_item_count].price = sqlite3_column_int(stmt, 2);
        hist_item_count++;
    }
    sqlite3_finalize(stmt);
}

#else
/* No SQLite - stub functions */
static int db_init(void) { load_default_menu(); return 0; }
static void db_load_menu(void) { /* already loaded */ }
static int db_authenticate(const char *user, const char *pass) {
    if(strcmp(user,"admin")==0 && strcmp(pass,"admin123")==0) {
        current_user_id=1; strcpy(current_username,"admin"); strcpy(current_role,"admin"); return 1;
    }
    if(strcmp(user,"kasir")==0 && strcmp(pass,"kasir123")==0) {
        current_user_id=2; strcpy(current_username,"kasir"); strcpy(current_role,"cashier"); return 1;
    }
    return 0;
}
static int db_save_order(void) { int n=onum; onum++; return n; }
static void db_load_users(void) { user_count=0; }
static void db_add_user(const char *u,const char *p,const char *r) { (void)u;(void)p;(void)r; }
static void db_update_user(int id,const char *u,const char *p,const char *r) { (void)id;(void)u;(void)p;(void)r; }
static void db_delete_user(int id) { (void)id; }
static void db_add_menu_item(const char *n,int p,int c) { (void)n;(void)p;(void)c; }
static void db_update_menu_item(int id,const char *n,int p,int c) { (void)id;(void)n;(void)p;(void)c; }
static void db_toggle_menu_item(int id) { (void)id; }
static void db_delete_menu_item(int id) { (void)id; }
static void db_load_order_history(void) { hist_count=0; }
static void db_load_order_detail(int id) { (void)id; hist_item_count=0; }
#endif

/* ===== Draw: Virtual Keyboard Overlay ===== */
static void draw_keyboard(FB *f, int y_start) {
    int W = f->w;
    int kw = 90, kh = 34, gap = 3;
    int row_w = 10 * kw + 9 * gap;
    int margin = (W - row_w) / 2;

    /* Dark background for keyboard area */
    int kb_h = f->h - y_start;
    rect(f, 0, y_start, W, kb_h, 25, 22, 15);
    hline(f, 0, y_start, W, C_GOLD_R, C_GOLD_G, C_GOLD_B);

    /* Input preview bar */
    int bar_h = 24;
    int bar_y = y_start + 4;
    rect(f, margin, bar_y, row_w, bar_h, C_BG_R, C_BG_G, C_BG_B);
    hline(f, margin, bar_y, row_w, C_GOLD_R, C_GOLD_G, C_GOLD_B);
    hline(f, margin, bar_y + bar_h - 1, row_w, C_GOLD_R, C_GOLD_G, C_GOLD_B);
    vline(f, margin, bar_y, bar_h, C_GOLD_R, C_GOLD_G, C_GOLD_B);
    vline(f, margin + row_w - 1, bar_y, bar_h, C_GOLD_R, C_GOLD_G, C_GOLD_B);

    if(kb_target_buf) {
        if(kb_password_mode) {
            char masked[64];
            int len = strlen(kb_target_buf);
            if(len > 63) len = 63;
            for(int i = 0; i < len; i++) masked[i] = '*';
            masked[len] = 0;
            text(f, margin + 6, bar_y + 4, masked, 1, C_TXT_R, C_TXT_G, C_TXT_B);
            /* cursor */
            int cx = margin + 6 + tw(masked, 1);
            rect(f, cx, bar_y + 3, 2, bar_h - 6, C_GOLD_R, C_GOLD_G, C_GOLD_B);
        } else {
            text(f, margin + 6, bar_y + 4, kb_target_buf, 1, C_TXT_R, C_TXT_G, C_TXT_B);
            int cx = margin + 6 + tw(kb_target_buf, 1);
            rect(f, cx, bar_y + 3, 2, bar_h - 6, C_GOLD_R, C_GOLD_G, C_GOLD_B);
        }
    }

    /* Close button [X] top-right of keyboard */
    {
        int xb_w = 40, xb_h = bar_h;
        int xb_x = margin + row_w - xb_w;
        rect(f, xb_x, bar_y, xb_w, xb_h, C_RED_R/2, C_RED_G/2, C_RED_B/2);
        hline(f, xb_x, bar_y, xb_w, C_RED_R, C_RED_G, C_RED_B);
        hline(f, xb_x, bar_y + xb_h - 1, xb_w, C_RED_R, C_RED_G, C_RED_B);
        vline(f, xb_x, bar_y, xb_h, C_RED_R, C_RED_G, C_RED_B);
        text_center(f, xb_x + xb_w/2, bar_y + (xb_h-16)/2, "X", 1, C_RED_R, C_RED_G, C_RED_B);
        if(nhit<256){hits[nhit]=(Hit){xb_x, bar_y, xb_w, xb_h, 644};nhit++;}  /* 644 = close keyboard */
    }

    int ky = bar_y + bar_h + 6;

    /* Row 1: 1 2 3 4 5 6 7 8 9 0 */
    {
        const char digits[] = "1234567890";
        for(int i = 0; i < 10; i++) {
            int kx = margin + i * (kw + gap);
            rect(f, kx, ky, kw, kh, C_PNL_R+10, C_PNL_G+10, C_PNL_B+5);
            hline(f, kx, ky, kw, C_BRD_R, C_BRD_G, C_BRD_B);
            hline(f, kx, ky+kh-1, kw, C_BRD_R, C_BRD_G, C_BRD_B);
            vline(f, kx, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
            vline(f, kx+kw-1, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
            char label[2] = {digits[i], 0};
            text_center(f, kx+kw/2, ky+(kh-16)/2, label, 1, C_GOLD_R, C_GOLD_G, C_GOLD_B);
            /* Action: 600-608 = '1'-'9', 609 = '0' */
            if(nhit<256){hits[nhit]=(Hit){kx,ky,kw,kh, 600+i};nhit++;}
        }
    }
    ky += kh + gap;

    /* Row 2: Q W E R T Y U I O P */
    {
        const char *upper = "QWERTYUIOP";
        const char *lower = "qwertyuiop";
        const char *row = kb_shift ? upper : lower;
        for(int i = 0; i < 10; i++) {
            int kx = margin + i * (kw + gap);
            rect(f, kx, ky, kw, kh, C_PNL_R+5, C_PNL_G+5, C_PNL_B+3);
            hline(f, kx, ky, kw, C_BRD_R, C_BRD_G, C_BRD_B);
            hline(f, kx, ky+kh-1, kw, C_BRD_R, C_BRD_G, C_BRD_B);
            vline(f, kx, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
            vline(f, kx+kw-1, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
            char label[2] = {row[i], 0};
            text_center(f, kx+kw/2, ky+(kh-16)/2, label, 1, C_TXT_R, C_TXT_G, C_TXT_B);
            /* Action: 610-619 */
            if(nhit<256){hits[nhit]=(Hit){kx,ky,kw,kh, 610+i};nhit++;}
        }
    }
    ky += kh + gap;

    /* Row 3: A S D F G H J K L  (9 keys, centered) */
    {
        const char *upper = "ASDFGHJKL";
        const char *lower = "asdfghjkl";
        const char *row = kb_shift ? upper : lower;
        int r3_margin = margin + (kw + gap) / 2;  /* offset half a key */
        for(int i = 0; i < 9; i++) {
            int kx = r3_margin + i * (kw + gap);
            rect(f, kx, ky, kw, kh, C_PNL_R+5, C_PNL_G+5, C_PNL_B+3);
            hline(f, kx, ky, kw, C_BRD_R, C_BRD_G, C_BRD_B);
            hline(f, kx, ky+kh-1, kw, C_BRD_R, C_BRD_G, C_BRD_B);
            vline(f, kx, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
            vline(f, kx+kw-1, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
            char label[2] = {row[i], 0};
            text_center(f, kx+kw/2, ky+(kh-16)/2, label, 1, C_TXT_R, C_TXT_G, C_TXT_B);
            /* Action: 620-628 */
            if(nhit<256){hits[nhit]=(Hit){kx,ky,kw,kh, 620+i};nhit++;}
        }
    }
    ky += kh + gap;

    /* Row 4: Z X C V B N M [BKSP]  (7 letters + backspace) */
    {
        const char *upper = "ZXCVBNM";
        const char *lower = "zxcvbnm";
        const char *row = kb_shift ? upper : lower;
        int r4_margin = margin + (kw + gap);  /* offset one key */
        for(int i = 0; i < 7; i++) {
            int kx = r4_margin + i * (kw + gap);
            rect(f, kx, ky, kw, kh, C_PNL_R+5, C_PNL_G+5, C_PNL_B+3);
            hline(f, kx, ky, kw, C_BRD_R, C_BRD_G, C_BRD_B);
            hline(f, kx, ky+kh-1, kw, C_BRD_R, C_BRD_G, C_BRD_B);
            vline(f, kx, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
            vline(f, kx+kw-1, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
            char label[2] = {row[i], 0};
            text_center(f, kx+kw/2, ky+(kh-16)/2, label, 1, C_TXT_R, C_TXT_G, C_TXT_B);
            /* Action: 629-635 */
            if(nhit<256){hits[nhit]=(Hit){kx,ky,kw,kh, 629+i};nhit++;}
        }
        /* BKSP button - wider */
        {
            int bksp_w = kw * 2 + gap;
            int kx = r4_margin + 7 * (kw + gap);
            rect(f, kx, ky, bksp_w, kh, C_RED_R/3, C_RED_G/3, C_RED_B/3);
            hline(f, kx, ky, bksp_w, C_RED_R, C_RED_G, C_RED_B);
            hline(f, kx, ky+kh-1, bksp_w, C_RED_R, C_RED_G, C_RED_B);
            vline(f, kx, ky, kh, C_RED_R, C_RED_G, C_RED_B);
            vline(f, kx+bksp_w-1, ky, kh, C_RED_R, C_RED_G, C_RED_B);
            text_center(f, kx+bksp_w/2, ky+(kh-16)/2, "BKSP", 1, C_RED_R, C_RED_G, C_RED_B);
            if(nhit<256){hits[nhit]=(Hit){kx,ky,bksp_w,kh, 641};nhit++;}
        }
    }
    ky += kh + gap;

    /* Row 5: [SHIFT] [____SPACE____] [ENTER] */
    {
        int shift_w = kw * 2 + gap;
        int enter_w = kw * 2 + gap;
        int space_w = row_w - shift_w - enter_w - 2 * gap;
        int kx = margin;

        /* SHIFT */
        rect(f, kx, ky, shift_w, kh,
             kb_shift ? C_GOLD_R/3 : C_PNL_R+10,
             kb_shift ? C_GOLD_G/3 : C_PNL_G+10,
             kb_shift ? C_GOLD_B/3 : C_PNL_B+5);
        hline(f, kx, ky, shift_w, kb_shift ? C_GOLD_R : C_BRD_R, kb_shift ? C_GOLD_G : C_BRD_G, kb_shift ? C_GOLD_B : C_BRD_B);
        hline(f, kx, ky+kh-1, shift_w, kb_shift ? C_GOLD_R : C_BRD_R, kb_shift ? C_GOLD_G : C_BRD_G, kb_shift ? C_GOLD_B : C_BRD_B);
        vline(f, kx, ky, kh, kb_shift ? C_GOLD_R : C_BRD_R, kb_shift ? C_GOLD_G : C_BRD_G, kb_shift ? C_GOLD_B : C_BRD_B);
        vline(f, kx+shift_w-1, ky, kh, kb_shift ? C_GOLD_R : C_BRD_R, kb_shift ? C_GOLD_G : C_BRD_G, kb_shift ? C_GOLD_B : C_BRD_B);
        text_center(f, kx+shift_w/2, ky+(kh-16)/2, "SHIFT", 1, kb_shift ? C_GOLD_R : C_DIM_R, kb_shift ? C_GOLD_G : C_DIM_G, kb_shift ? C_GOLD_B : C_DIM_B);
        if(nhit<256){hits[nhit]=(Hit){kx,ky,shift_w,kh, 643};nhit++;}
        kx += shift_w + gap;

        /* SPACE */
        rect(f, kx, ky, space_w, kh, C_PNL_R+10, C_PNL_G+10, C_PNL_B+5);
        hline(f, kx, ky, space_w, C_BRD_R, C_BRD_G, C_BRD_B);
        hline(f, kx, ky+kh-1, space_w, C_BRD_R, C_BRD_G, C_BRD_B);
        vline(f, kx, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
        vline(f, kx+space_w-1, ky, kh, C_BRD_R, C_BRD_G, C_BRD_B);
        text_center(f, kx+space_w/2, ky+(kh-16)/2, "SPACE", 1, C_DIM_R, C_DIM_G, C_DIM_B);
        if(nhit<256){hits[nhit]=(Hit){kx,ky,space_w,kh, 640};nhit++;}
        kx += space_w + gap;

        /* ENTER */
        rect(f, kx, ky, enter_w, kh, C_GRN_R/2, C_GRN_G/2, C_GRN_B/2);
        hline(f, kx, ky, enter_w, C_GRN_R, C_GRN_G, C_GRN_B);
        hline(f, kx, ky+kh-1, enter_w, C_GRN_R, C_GRN_G, C_GRN_B);
        vline(f, kx, ky, kh, C_GRN_R, C_GRN_G, C_GRN_B);
        vline(f, kx+enter_w-1, ky, kh, C_GRN_R, C_GRN_G, C_GRN_B);
        text_center(f, kx+enter_w/2, ky+(kh-16)/2, "ENTER", 1, C_GRN_R, C_GRN_G, C_GRN_B);
        if(nhit<256){hits[nhit]=(Hit){kx,ky,enter_w,kh, 642};nhit++;}
    }
}

/* ===== Draw: Back Button (top-left on sub-pages) ===== */
static void draw_back_button(FB *f, int y) {
    int bw = 80, bh = 36;
    int bx = 10, by = y;
    rect(f, bx, by, bw, bh, C_PNL_R+10, C_PNL_G+10, C_PNL_B+5);
    hline(f, bx, by, bw, C_GOLD_R, C_GOLD_G, C_GOLD_B);
    hline(f, bx, by+bh-1, bw, C_GOLD_R, C_GOLD_G, C_GOLD_B);
    vline(f, bx, by, bh, C_GOLD_R, C_GOLD_G, C_GOLD_B);
    vline(f, bx+bw-1, by, bh, C_GOLD_R, C_GOLD_G, C_GOLD_B);
    text_center(f, bx+bw/2, by+(bh-16)/2, "<BACK", 1, C_GOLD_R, C_GOLD_G, C_GOLD_B);
    if(nhit<256){hits[nhit]=(Hit){bx, by, bw, bh, 710};nhit++;}
}

/* ===== Draw: Navigation Buttons (POS header area) ===== */
static void draw_nav_buttons(FB *f, int y) {
    const char *labels[] = {"MENU", "USERS", "HISTORY", "HELP", "LOGOUT"};
    int actions[] = {700, 701, 702, 703, 704};
    int nbtns = 5;
    int W = f->w;
    int RPW_nav = W*42/100;
    int LPW_nav = W - RPW_nav - 2;
    int gap = 6;
    int btn_w = (LPW_nav - 12 - (nbtns - 1) * gap) / nbtns;
    int btn_h = 30;
    int total_w = nbtns * btn_w + (nbtns - 1) * gap;
    int start_x = (LPW_nav - total_w) / 2;

    /* Background strip - only left panel */
    rect(f, 0, y, LPW_nav, btn_h + 6, C_PNL_R, C_PNL_G, C_PNL_B);
    hline(f, 0, y + btn_h + 5, LPW_nav, C_BRD_R, C_BRD_G, C_BRD_B);

    for(int i = 0; i < nbtns; i++) {
        int bx = start_x + i * (btn_w + gap);
        int by = y + 3;

        /* Admin-only: dim USERS button for non-admins */
        int is_dim = (i == 1 && strcmp(current_role, "admin") != 0);

        rect(f, bx, by, btn_w, btn_h,
             is_dim ? C_PNL_R : C_PNL_R+10,
             is_dim ? C_PNL_G : C_PNL_G+10,
             is_dim ? C_PNL_B : C_PNL_B+5);
        hline(f, bx, by, btn_w,
              is_dim ? C_BRD_R : C_GOLD_R,
              is_dim ? C_BRD_G : C_GOLD_G,
              is_dim ? C_BRD_B : C_GOLD_B);
        hline(f, bx, by+btn_h-1, btn_w,
              is_dim ? C_BRD_R : C_GOLD_R,
              is_dim ? C_BRD_G : C_GOLD_G,
              is_dim ? C_BRD_B : C_GOLD_B);
        vline(f, bx, by, btn_h,
              is_dim ? C_BRD_R : C_GOLD_R,
              is_dim ? C_BRD_G : C_GOLD_G,
              is_dim ? C_BRD_B : C_GOLD_B);
        vline(f, bx+btn_w-1, by, btn_h,
              is_dim ? C_BRD_R : C_GOLD_R,
              is_dim ? C_BRD_G : C_GOLD_G,
              is_dim ? C_BRD_B : C_GOLD_B);
        text_center(f, bx+btn_w/2, by+(btn_h-16)/2, labels[i], 1,
                    is_dim ? C_DIM_R/2 : C_GOLD_R,
                    is_dim ? C_DIM_G/2 : C_GOLD_G,
                    is_dim ? C_DIM_B/2 : C_GOLD_B);
        if(nhit<256){hits[nhit]=(Hit){bx, by, btn_w, btn_h, actions[i]};nhit++;}
    }
}

/* ===== Draw: F1 Help Overlay ===== */
static void draw_help_overlay(FB *f) {
    int W=f->w, H=f->h;
    /* Semi-transparent dark overlay (darken by drawing translucent rect) */
    int ox=W/2-280, oy=H/2-200, ow=560, oh=400;
    if(ox<10) ox=10; if(oy<10) oy=10;
    if(ox+ow>W-10) ow=W-20-ox; if(oy+oh>H-10) oh=H-20-oy;

    /* Dark background */
    rect(f,ox,oy,ow,oh,15,12,8);
    /* Border */
    hline(f,ox,oy,ow,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    hline(f,ox,oy+oh-1,ow,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    vline(f,ox,oy,oh,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    vline(f,ox+ow-1,oy,oh,C_GOLD_R,C_GOLD_G,C_GOLD_B);

    int cx=ox+ow/2;
    int y=oy+12;
    text_center(f,cx,y,"KEYBOARD SHORTCUTS",2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    y+=38;
    hline(f,ox+20,y,ow-40,C_BRD_R,C_BRD_G,C_BRD_B);
    y+=12;

    /* Two columns */
    int lx=ox+30, rx2=ox+ow/2+20;

    text(f,lx,y,"ORDER",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    text(f,rx2,y,"NAVIGATION",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    y+=22;
    text(f,lx,y,"1-9    Add item to order",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"Tab    Cycle categories",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"-      Decrease qty",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"Up/Dn  Select order item",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"Bksp   Remove item",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"Lt/Rt  Menu page nav",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"0      Clear order",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"PgUp   Previous page",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"Enter  Pay / confirm",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"PgDn   Next page",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=28;

    text(f,lx,y,"PAYMENT",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    text(f,rx2,y,"MANAGEMENT",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    y+=22;
    text(f,lx,y,"C      Cash payment",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"M      Menu management",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"R      QRIS payment",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"U      User management",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"H      Order history",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"L      Logout",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=28;

    text(f,lx,y,"SYSTEM",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    text(f,rx2,y,"TOUCH",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    y+=22;
    text(f,lx,y,"F1     Toggle this help",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"Tap card  Add to order",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"ESC    Exit application",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"[+][-] Qty on order",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=20;
    text(f,lx,y,"",1,C_TXT_R,C_TXT_G,C_TXT_B);
    text(f,rx2,y,"< >    Page navigation",1,C_TXT_R,C_TXT_G,C_TXT_B);
    y+=28;

    text_center(f,cx,oy+oh-24,"Press F1 or ESC to close",1,C_DIM_R,C_DIM_G,C_DIM_B);
}

/* ===== Draw: POS Dashboard ===== */
static void draw_pos(FB *f) {
    int W=f->w, H=f->h;
    nhit=0;
    char buf[64];

    /* Layout constants */
    int NAV_H=36;  /* navigation buttons row height */
    int HDR=40, FTR=20, CAT_H=44; /* 44px for touch-friendly tabs */
    int RPW = W*42/100;         /* right panel width (~430px for 1024) */
    int LPW = W - RPW - 2;     /* left panel width (~592px for 1024) */
    int BODY_Y = HDR;
    int BODY_H = H - HDR - FTR;

    /* Background */
    rect(f,0,0,W,H,C_BG_R,C_BG_G,C_BG_B);

    /* -- Header -- */
    rect(f,0,0,W,HDR,C_PNL_R,C_PNL_G,C_PNL_B);
    hline(f,0,HDR-1,W,C_BRD_R,C_BRD_G,C_BRD_B);
    text(f,10,12,"WAYANG POS",2,C_GOLD_R,C_GOLD_G,C_GOLD_B);

    /* Show logged in user */
    if(current_username[0]) {
        snprintf(buf,sizeof(buf),"[%s]",current_username);
        text(f,W/3,16,buf,1,C_DIM_R,C_DIM_G,C_DIM_B);
    }

    time_t now=time(NULL); struct tm *ti=localtime(&now);
    char clk[8],dt[12];
    strftime(clk,sizeof(clk),"%H:%M",ti);
    strftime(dt,sizeof(dt),"%b %d",ti);
    text(f,W-tw(clk,2)-tw(dt,1)-25,10,clk,2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    text(f,W-tw(dt,1)-8,16,dt,1,C_DIM_R,C_DIM_G,C_DIM_B);

    /* ── Navigation Buttons Row ── */
    draw_nav_buttons(f, HDR);

    /* ── Left: Category tabs at top ── */
    int cat_y = BODY_Y + NAV_H;
    rect(f,0,cat_y,LPW,CAT_H,C_PNL_R,C_PNL_G,C_PNL_B);
    hline(f,0,cat_y+CAT_H-1,LPW,C_BRD_R,C_BRD_G,C_BRD_B);
    int tabw=LPW/4;
    for(int i=0;i<4;i++){
        int tx=i*tabw;
        if(i==cur_cat){
            /* Active tab: filled background + gold underline */
            rect(f,tx+2,cat_y+2,tabw-4,CAT_H-4,C_SEL_R,C_SEL_G,C_SEL_B);
            hline(f,tx+2,cat_y+CAT_H-3,tabw-4,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            hline(f,tx+2,cat_y+CAT_H-4,tabw-4,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            text_center(f,tx+tabw/2,cat_y+(CAT_H-16)/2,cats[i],1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
        } else {
            /* Inactive tabs: brighter text (not so dim) */
            text_center(f,tx+tabw/2,cat_y+(CAT_H-16)/2,cats[i],1,C_TXT_R,C_TXT_G,C_TXT_B);
        }
        if(nhit<256){hits[nhit]=(Hit){tx,cat_y,tabw,CAT_H,100+i};nhit++;}
    }

    /* -- Left: Menu grid with pagination -- */
    int cols=3, rows=3, gap=6;
    int PAGE_INDICATOR_H = 22;
    int grid_y = cat_y + CAT_H + 4;
    int grid_bottom = H - FTR - PAGE_INDICATOR_H - 4;
    int available_h = grid_bottom - grid_y;
    int cw=(LPW - 12 - (cols-1)*gap)/cols;
    /* Dynamic card height: fill vertical space */
    int ch=(available_h - (rows-1)*gap) / rows;
    if(ch < 50) ch = 50; /* minimum card height */

    /* Visible items based on category filter (active only) */
    int vis[64], nvis=0;
    for(int i=0;i<menu_count;i++)
        if(menu_items[i].active && (cur_cat==0||menu_items[i].cat==cur_cat-1))
            vis[nvis++]=i;

    /* Pagination */
    int items_per_page = 9;
    int total_pages = (nvis + items_per_page - 1) / items_per_page;
    if(total_pages < 1) total_pages = 1;
    if(menu_page >= total_pages) menu_page = total_pages - 1;
    if(menu_page < 0) menu_page = 0;
    int page_start = menu_page * items_per_page;

    /* Category border colors: gold=drinks(0), orange=food(1), amber=snacks(2) */
    int cat_border_r[] = {220, 220, 180};
    int cat_border_g[] = {180,  120, 140};
    int cat_border_b[] = { 40,  20,  20};

    for(int v=0;v<items_per_page&&(page_start+v)<nvis;v++){
        int i=vis[page_start+v];
        int col=v%cols, row=v/cols;
        int cx=6+col*(cw+gap);
        int cy=grid_y+row*(ch+gap);

        /* Card bg */
        int sel=0;
        for(int j=0;j<ocnt;j++) if(order[j].idx==i){sel=order[j].qty;break;}

        rect(f,cx,cy,cw,ch, sel?C_SEL_R:C_PNL_R, sel?C_SEL_G:C_PNL_G, sel?C_SEL_B:C_PNL_B);

        /* Border */
        hline(f,cx,cy,cw,C_BRD_R,C_BRD_G,C_BRD_B);
        hline(f,cx,cy+ch-1,cw,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,cx+cw-1,cy,ch,C_BRD_R,C_BRD_G,C_BRD_B);

        /* Colored left border per category */
        int mcat = menu_items[i].cat;
        if(mcat<0) mcat=0; if(mcat>2) mcat=2;
        rect(f,cx,cy,3,ch,cat_border_r[mcat],cat_border_g[mcat],cat_border_b[mcat]);

        /* Number badge in top-left (gold bg, dark text) */
        snprintf(buf,sizeof(buf),"%d",v+1);
        int badge_w=20, badge_h=18;
        rect(f,cx+5,cy+5,badge_w,badge_h,C_GOLD_R,C_GOLD_G,C_GOLD_B);
        draw_char(f,cx+5+(badge_w-8)/2,cy+5+1,buf[0],1,C_BG_R,C_BG_G,C_BG_B);

        /* Name - scale 1 to avoid truncation */
        /* Truncate name to fit card width at scale 1 */
        char name_trunc[32];
        int name_area_w = cw - 32; /* leave room for badge + margin */
        int max_name_chars = name_area_w / (8+1);
        if(max_name_chars > 31) max_name_chars = 31;
        if(max_name_chars < 3) max_name_chars = 3;
        strncpy(name_trunc, menu_items[i].name, max_name_chars);
        name_trunc[max_name_chars] = 0;
        text(f,cx+28,cy+6,name_trunc,1,C_TXT_R,C_TXT_G,C_TXT_B);

        /* Price - right-aligned, gold, scale 1 */
        fmt_rp(buf,sizeof(buf),menu_items[i].price);
        text_right(f,cx+cw-6,cy+ch-22,buf,1,C_GOLD_R,C_GOLD_G,C_GOLD_B);

        /* Qty badge in top-right */
        if(sel>0){
            snprintf(buf,sizeof(buf),"%d",sel);
            rect(f,cx+cw-24,cy+5,20,18,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            draw_char(f,cx+cw-19,cy+6,buf[0],1,C_BG_R,C_BG_G,C_BG_B);
        }

        if(nhit<256){hits[nhit]=(Hit){cx,cy,cw,ch,i};nhit++;}
    }

    /* Page indicator with touch-friendly < > arrow buttons */
    {
        int pi_y = grid_bottom + 2;
        int arrow_w = 44, arrow_h = 20;
        if(total_pages > 1) {
            /* Left arrow button */
            int lax = 6;
            if(menu_page > 0) {
                rect(f,lax,pi_y,arrow_w,arrow_h,C_PNL_R+10,C_PNL_G+10,C_PNL_B+5);
                hline(f,lax,pi_y,arrow_w,C_BRD_R,C_BRD_G,C_BRD_B);
                hline(f,lax,pi_y+arrow_h-1,arrow_w,C_BRD_R,C_BRD_G,C_BRD_B);
                vline(f,lax,pi_y,arrow_h,C_BRD_R,C_BRD_G,C_BRD_B);
                vline(f,lax+arrow_w-1,pi_y,arrow_h,C_BRD_R,C_BRD_G,C_BRD_B);
                text_center(f,lax+arrow_w/2,pi_y+2,"<",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            } else {
                rect(f,lax,pi_y,arrow_w,arrow_h,C_PNL_R,C_PNL_G,C_PNL_B);
                text_center(f,lax+arrow_w/2,pi_y+2,"<",1,C_DIM_R/2,C_DIM_G/2,C_DIM_B/2);
            }
            if(nhit<256){hits[nhit]=(Hit){lax,pi_y,arrow_w,arrow_h,501};nhit++;}

            /* Page number */
            snprintf(buf,sizeof(buf),"Page %d/%d",menu_page+1,total_pages);
            text_center(f,LPW/2,pi_y+2,buf,1,C_GOLD_R,C_GOLD_G,C_GOLD_B);

            /* Right arrow button */
            int rax = LPW - 6 - arrow_w;
            if(menu_page < total_pages-1) {
                rect(f,rax,pi_y,arrow_w,arrow_h,C_PNL_R+10,C_PNL_G+10,C_PNL_B+5);
                hline(f,rax,pi_y,arrow_w,C_BRD_R,C_BRD_G,C_BRD_B);
                hline(f,rax,pi_y+arrow_h-1,arrow_w,C_BRD_R,C_BRD_G,C_BRD_B);
                vline(f,rax,pi_y,arrow_h,C_BRD_R,C_BRD_G,C_BRD_B);
                vline(f,rax+arrow_w-1,pi_y,arrow_h,C_BRD_R,C_BRD_G,C_BRD_B);
                text_center(f,rax+arrow_w/2,pi_y+2,">",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            } else {
                rect(f,rax,pi_y,arrow_w,arrow_h,C_PNL_R,C_PNL_G,C_PNL_B);
                text_center(f,rax+arrow_w/2,pi_y+2,">",1,C_DIM_R/2,C_DIM_G/2,C_DIM_B/2);
            }
            if(nhit<256){hits[nhit]=(Hit){rax,pi_y,arrow_w,arrow_h,502};nhit++;}
        }
    }

    /* -- Right Panel: Order -- */
    int rx=LPW+2;
    rect(f,rx,BODY_Y,RPW,BODY_H,C_PNL_R,C_PNL_G,C_PNL_B);
    vline(f,rx,BODY_Y,BODY_H,C_BRD_R,C_BRD_G,C_BRD_B);

    /* Order header */
    snprintf(buf,sizeof(buf),"ORDER #%04d",onum);
    text(f,rx+12,BODY_Y+8,buf,1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    snprintf(buf,sizeof(buf),"%d items",order_count_items());
    text_right(f,W-10,BODY_Y+8,buf,1,C_DIM_R,C_DIM_G,C_DIM_B);

    int oy=BODY_Y+26;
    hline(f,rx+8,oy,RPW-16,C_BRD_R,C_BRD_G,C_BRD_B);
    oy+=6;

    /* -- Bottom section heights (fixed, computed upward from footer) -- */
    int pay_h = 48;  /* taller PAY button for touch */
    int btn_h = 36;  /* taller CASH/QRIS for touch */
    int clear_h = 36; /* CLEAR ORDER button */
    int total_section_h = 18*3 + 4 + 16 + 4 + btn_h + 4 + pay_h + 4 + clear_h + 8;

    int max_order_y = H - FTR - total_section_h;
    int order_line_h = 36; /* touch-friendly line height */
    int btn_sz = 36; /* +/- button size (touch-friendly) */

    /* Clamp order_scroll */
    int max_visible = (max_order_y - oy) / order_line_h;
    if(max_visible < 1) max_visible = 1;
    if(order_scroll > ocnt - max_visible) order_scroll = ocnt - max_visible;
    if(order_scroll < 0) order_scroll = 0;

    /* Order scroll up arrow */
    if(order_scroll > 0) {
        int arrow_w = RPW - 16, arrow_h = 20;
        rect(f,rx+8,oy,arrow_w,arrow_h,C_PNL_R+10,C_PNL_G+10,C_PNL_B+5);
        text_center(f,rx+RPW/2,oy+2,"^ ^ ^",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
        if(nhit<256){hits[nhit]=(Hit){rx+8,oy,arrow_w,arrow_h,503};nhit++;}
        oy += arrow_h + 2;
        max_visible = (max_order_y - oy) / order_line_h;
        if(max_visible < 1) max_visible = 1;
    }

    /* Empty order placeholder */
    if(ocnt == 0) {
        int empty_cy = oy + (max_order_y - oy) / 2;
        text_center(f,rx+RPW/2,empty_cy,"Tap menu to add items",1,C_DIM_R,C_DIM_G,C_DIM_B);
    }

    /* Order line items with selection highlight and +/- buttons */
    int drawn_lines = 0;
    for(int i=order_scroll;i<ocnt&&drawn_lines<max_visible;i++){
        int idx=order[i].idx;

        /* Highlight selected line */
        if(i == order_sel) {
            rect(f,rx+2,oy,RPW-4,order_line_h-2,C_HI_R,C_HI_G,C_HI_B);
        }

        /* [-] button */
        int minus_x = W - 10 - btn_sz;
        int minus_y = oy + (order_line_h - btn_sz) / 2;
        rect(f,minus_x,minus_y,btn_sz,btn_sz,C_RED_R/2,C_RED_G/2,C_RED_B/2);
        hline(f,minus_x,minus_y,btn_sz,C_RED_R,C_RED_G,C_RED_B);
        hline(f,minus_x,minus_y+btn_sz-1,btn_sz,C_RED_R,C_RED_G,C_RED_B);
        vline(f,minus_x,minus_y,btn_sz,C_RED_R,C_RED_G,C_RED_B);
        vline(f,minus_x+btn_sz-1,minus_y,btn_sz,C_RED_R,C_RED_G,C_RED_B);
        text_center(f,minus_x+btn_sz/2,minus_y+(btn_sz-16)/2,"-",1,C_RED_R,C_RED_G,C_RED_B);
        if(nhit<256){hits[nhit]=(Hit){minus_x,minus_y,btn_sz,btn_sz,400+i};nhit++;}

        /* [+] button */
        int plus_x = minus_x - btn_sz - 4;
        int plus_y = minus_y;
        rect(f,plus_x,plus_y,btn_sz,btn_sz,C_GRN_R/3,C_GRN_G/3,C_GRN_B/3);
        hline(f,plus_x,plus_y,btn_sz,C_GRN_R,C_GRN_G,C_GRN_B);
        hline(f,plus_x,plus_y+btn_sz-1,btn_sz,C_GRN_R,C_GRN_G,C_GRN_B);
        vline(f,plus_x,plus_y,btn_sz,C_GRN_R,C_GRN_G,C_GRN_B);
        vline(f,plus_x+btn_sz-1,plus_y,btn_sz,C_GRN_R,C_GRN_G,C_GRN_B);
        text_center(f,plus_x+btn_sz/2,plus_y+(btn_sz-16)/2,"+",1,C_GRN_R,C_GRN_G,C_GRN_B);
        if(nhit<256){hits[nhit]=(Hit){plus_x,plus_y,btn_sz,btn_sz,300+i};nhit++;}

        /* Item text */
        snprintf(buf,sizeof(buf),"%dx %s",order[i].qty,menu_items[idx].name);
        /* Truncate to fit before +/- buttons */
        int max_text_w = plus_x - rx - 18;
        int max_chars = max_text_w / 9;
        if(max_chars > 0 && max_chars < (int)strlen(buf)) buf[max_chars] = 0;
        text(f,rx+12,oy+(order_line_h-16)/2,buf,1, i==order_sel?C_GOLD_R:C_TXT_R, i==order_sel?C_GOLD_G:C_TXT_G, i==order_sel?C_GOLD_B:C_TXT_B);

        /* Subtotal below item name (small) */
        int sub=order[i].qty*menu_items[idx].price;
        fmt_rp(buf,sizeof(buf),sub);
        text(f,rx+12,oy+(order_line_h-16)/2+14,buf,1, i==order_sel?C_GOLD_R:C_AMB_R, i==order_sel?C_GOLD_G:C_AMB_G, i==order_sel?C_GOLD_B:C_AMB_B);

        oy+=order_line_h;
        drawn_lines++;
    }

    /* Order scroll down arrow */
    if(order_scroll + drawn_lines < ocnt) {
        int arrow_w = RPW - 16, arrow_h = 20;
        if(oy + arrow_h <= max_order_y) {
            rect(f,rx+8,oy,arrow_w,arrow_h,C_PNL_R+10,C_PNL_G+10,C_PNL_B+5);
            text_center(f,rx+RPW/2,oy+2,"v v v",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            if(nhit<256){hits[nhit]=(Hit){rx+8,oy,arrow_w,arrow_h,504};nhit++;}
        }
    }

    /* -- Totals + Buttons (anchored from bottom) -- */
    int total=order_total();
    int tax=total*11/100;
    int grand=total+tax;

    int ftr_top = H - FTR;

    /* PAY button - tall, full-width of order panel, brighter when amount > 0 */
    int pay_y = ftr_top - pay_h - 4;
    if(grand > 0) {
        rect(f,rx+4,pay_y,RPW-8,pay_h,C_GOLD_R,C_GOLD_G,C_GOLD_B);
        fmt_rp(buf,sizeof(buf),grand);
        char pay[48]; snprintf(pay,sizeof(pay),"PAY %s",buf);
        text_center(f,rx+RPW/2,pay_y+(pay_h-32)/2,pay,2,C_BG_R,C_BG_G,C_BG_B);
    } else {
        rect(f,rx+4,pay_y,RPW-8,pay_h,C_PNL_R+10,C_PNL_G+10,C_PNL_B+5);
        hline(f,rx+4,pay_y,RPW-8,C_BRD_R,C_BRD_G,C_BRD_B);
        hline(f,rx+4,pay_y+pay_h-1,RPW-8,C_BRD_R,C_BRD_G,C_BRD_B);
        text_center(f,rx+RPW/2,pay_y+(pay_h-32)/2,"PAY Rp 0",2,C_DIM_R,C_DIM_G,C_DIM_B);
    }
    if(nhit<256){hits[nhit]=(Hit){rx+4,pay_y,RPW-8,pay_h,202};nhit++;}

    /* CLEAR ORDER button - red/warning, above PAY */
    int clear_y = pay_y - clear_h - 4;
    if(ocnt > 0) {
        rect(f,rx+4,clear_y,RPW-8,clear_h,C_RED_R/2,C_RED_G/2,C_RED_B/2);
        hline(f,rx+4,clear_y,RPW-8,C_RED_R,C_RED_G,C_RED_B);
        hline(f,rx+4,clear_y+clear_h-1,RPW-8,C_RED_R,C_RED_G,C_RED_B);
        vline(f,rx+4,clear_y,clear_h,C_RED_R,C_RED_G,C_RED_B);
        vline(f,rx+RPW-5,clear_y,clear_h,C_RED_R,C_RED_G,C_RED_B);
        text_center(f,rx+RPW/2,clear_y+(clear_h-16)/2,"CLEAR ORDER",1,C_RED_R,C_RED_G,C_RED_B);
    } else {
        rect(f,rx+4,clear_y,RPW-8,clear_h,C_PNL_R,C_PNL_G,C_PNL_B);
        hline(f,rx+4,clear_y,RPW-8,C_BRD_R,C_BRD_G,C_BRD_B);
        hline(f,rx+4,clear_y+clear_h-1,RPW-8,C_BRD_R,C_BRD_G,C_BRD_B);
        text_center(f,rx+RPW/2,clear_y+(clear_h-16)/2,"CLEAR ORDER",1,C_DIM_R/2,C_DIM_G/2,C_DIM_B/2);
    }
    if(nhit<256){hits[nhit]=(Hit){rx+4,clear_y,RPW-8,clear_h,500};nhit++;}

    /* CASH + QRIS buttons - touch-friendly height */
    int btn_y = clear_y - btn_h - 4;
    int bw = (RPW - 20) / 2;
    if(pay_method == 0) {
        rect(f,rx+4,btn_y,bw,btn_h,C_GRN_R,C_GRN_G,C_GRN_B);
        text_center(f,rx+4+bw/2,btn_y+(btn_h-16)/2,"CASH",1,255,255,255);
        rect(f,rx+8+bw,btn_y,bw,btn_h,C_PNL_R+10,C_PNL_G+10,C_PNL_B+5);
        hline(f,rx+8+bw,btn_y,bw,C_BRD_R,C_BRD_G,C_BRD_B);
        hline(f,rx+8+bw,btn_y+btn_h-1,bw,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,rx+8+bw,btn_y,btn_h,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,rx+8+bw+bw-1,btn_y,btn_h,C_BRD_R,C_BRD_G,C_BRD_B);
        text_center(f,rx+8+bw+bw/2,btn_y+(btn_h-16)/2,"QRIS",1,C_DIM_R,C_DIM_G,C_DIM_B);
    } else {
        rect(f,rx+4,btn_y,bw,btn_h,C_PNL_R+10,C_PNL_G+10,C_PNL_B+5);
        hline(f,rx+4,btn_y,bw,C_BRD_R,C_BRD_G,C_BRD_B);
        hline(f,rx+4,btn_y+btn_h-1,bw,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,rx+4,btn_y,btn_h,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,rx+4+bw-1,btn_y,btn_h,C_BRD_R,C_BRD_G,C_BRD_B);
        text_center(f,rx+4+bw/2,btn_y+(btn_h-16)/2,"CASH",1,C_DIM_R,C_DIM_G,C_DIM_B);
        rect(f,rx+8+bw,btn_y,bw,btn_h,C_BLU_R,C_BLU_G,C_BLU_B);
        text_center(f,rx+8+bw+bw/2,btn_y+(btn_h-16)/2,"QRIS",1,255,255,255);
    }
    if(nhit<256){hits[nhit]=(Hit){rx+4,btn_y,bw,btn_h,200};nhit++;}
    if(nhit<256){hits[nhit]=(Hit){rx+8+bw,btn_y,bw,btn_h,201};nhit++;}

    /* PAYMENT label */
    int lbl_y = btn_y - 16;
    text(f,rx+8,lbl_y,"PAYMENT",1,C_DIM_R,C_DIM_G,C_DIM_B);

    /* Divider */
    hline(f,rx+6,lbl_y-4,RPW-12,C_BRD_R,C_BRD_G,C_BRD_B);

    /* TOTAL */
    int tot_y = lbl_y - 4 - 22;
    fmt_rp(buf,sizeof(buf),grand);
    text(f,rx+8,tot_y,"TOTAL",2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    text_right(f,W-10,tot_y,buf,2,C_GOLD_R,C_GOLD_G,C_GOLD_B);

    /* Tax */
    int tax_y = tot_y - 18;
    fmt_rp(buf,sizeof(buf),tax);
    text(f,rx+8,tax_y,"TAX 11%",1,C_DIM_R,C_DIM_G,C_DIM_B);
    text_right(f,W-10,tax_y,buf,1,C_TXT_R,C_TXT_G,C_TXT_B);

    /* Subtotal */
    int sub_y = tax_y - 18;
    fmt_rp(buf,sizeof(buf),total);
    text(f,rx+8,sub_y,"SUBTOTAL",1,C_DIM_R,C_DIM_G,C_DIM_B);
    text_right(f,W-10,sub_y,buf,1,C_TXT_R,C_TXT_G,C_TXT_B);

    /* Divider above subtotal */
    hline(f,rx+6,sub_y-6,RPW-12,C_BRD_R,C_BRD_G,C_BRD_B);

    /* -- Single-line Footer / Status Bar -- */
    rect(f,0,H-FTR,W,FTR,C_PNL_R,C_PNL_G,C_PNL_B);
    hline(f,0,H-FTR,W,C_BRD_R,C_BRD_G,C_BRD_B);
    {
        char status[128];
        snprintf(status,sizeof(status),"wayang:pos | %s | %dx%d | Page %d/%d | [F1] Help",
            current_username[0]?current_username:"?", W, H, menu_page+1, total_pages);
        text(f,8,H-FTR+3,status,1,C_DIM_R,C_DIM_G,C_DIM_B);
    }

    /* -- F1 Help Overlay -- */
    if(show_help) {
        draw_help_overlay(f);
    }

    /* -- Virtual Keyboard Overlay -- */
    if(kb_visible) {
        draw_keyboard(f, H - 210);
    }
}

/* ===== Draw: Login Screen ===== */
static void draw_login(FB *f) {
    int W=f->w, H=f->h;
    nhit=0;

    rect(f,0,0,W,H,C_BG_R,C_BG_G,C_BG_B);

    /* Title */
    text_center(f,W/2,80,"W A Y A N G  P O S",2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    int sepw=300;
    rect(f,W/2-sepw/2,120,sepw,2,C_BRD_R,C_BRD_G,C_BRD_B);

    text_center(f,W/2,150,"Login",2,C_TXT_R,C_TXT_G,C_TXT_B);

    /* Form box */
    int bx=W/2-160, by=190, bw=320, bh=260;
    rect(f,bx,by,bw,bh,C_PNL_R,C_PNL_G,C_PNL_B);
    hline(f,bx,by,bw,C_BRD_R,C_BRD_G,C_BRD_B);
    hline(f,bx,by+bh-1,bw,C_BRD_R,C_BRD_G,C_BRD_B);
    vline(f,bx,by,bh,C_BRD_R,C_BRD_G,C_BRD_B);
    vline(f,bx+bw-1,by,bh,C_BRD_R,C_BRD_G,C_BRD_B);

    /* Username field */
    int fy = by + 20;
    text(f,bx+20,fy,"Username:",1, login_field==0?C_GOLD_R:C_DIM_R, login_field==0?C_GOLD_G:C_DIM_G, login_field==0?C_GOLD_B:C_DIM_B);
    fy += 20;
    int fx=bx+20, fw=bw-40, fh=28; /* taller input fields for touch */
    rect(f,fx,fy,fw,fh, login_field==0?C_SEL_R:C_BG_R, login_field==0?C_SEL_G:C_BG_G, login_field==0?C_SEL_B:C_BG_B);
    hline(f,fx,fy,fw, login_field==0?C_GOLD_R:C_BRD_R, login_field==0?C_GOLD_G:C_BRD_G, login_field==0?C_GOLD_B:C_BRD_B);
    hline(f,fx,fy+fh-1,fw, login_field==0?C_GOLD_R:C_BRD_R, login_field==0?C_GOLD_G:C_BRD_G, login_field==0?C_GOLD_B:C_BRD_B);
    text(f,fx+4,fy+6,login_user,1,C_TXT_R,C_TXT_G,C_TXT_B);
    /* Cursor blink */
    if(login_field==0) {
        int cx=fx+4+tw(login_user,1);
        rect(f,cx,fy+4,2,fh-8,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    }
    /* Hit region for username field */
    if(nhit<256){hits[nhit]=(Hit){fx,fy,fw,fh,506};nhit++;}

    /* Password field */
    fy += 40;
    text(f,bx+20,fy,"Password:",1, login_field==1?C_GOLD_R:C_DIM_R, login_field==1?C_GOLD_G:C_DIM_G, login_field==1?C_GOLD_B:C_DIM_B);
    fy += 20;
    rect(f,fx,fy,fw,fh, login_field==1?C_SEL_R:C_BG_R, login_field==1?C_SEL_G:C_BG_G, login_field==1?C_SEL_B:C_BG_B);
    hline(f,fx,fy,fw, login_field==1?C_GOLD_R:C_BRD_R, login_field==1?C_GOLD_G:C_BRD_G, login_field==1?C_GOLD_B:C_BRD_B);
    hline(f,fx,fy+fh-1,fw, login_field==1?C_GOLD_R:C_BRD_R, login_field==1?C_GOLD_G:C_BRD_G, login_field==1?C_GOLD_B:C_BRD_B);
    /* Mask password */
    char masked[32];
    int plen=strlen(login_pass);
    for(int i=0;i<plen&&i<31;i++) masked[i]='*';
    masked[plen]=0;
    text(f,fx+4,fy+6,masked,1,C_TXT_R,C_TXT_G,C_TXT_B);
    if(login_field==1) {
        int cx=fx+4+tw(masked,1);
        rect(f,cx,fy+4,2,fh-8,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    }
    /* Hit region for password field */
    if(nhit<256){hits[nhit]=(Hit){fx,fy,fw,fh,507};nhit++;}

    /* LOGIN button - large touch target */
    fy += 35;
    int login_btn_w = bw - 40, login_btn_h = 40;
    int login_btn_x = bx + 20, login_btn_y = fy;
    rect(f,login_btn_x,login_btn_y,login_btn_w,login_btn_h,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    text_center(f,bx+bw/2,login_btn_y+(login_btn_h-32)/2,"LOGIN",2,C_BG_R,C_BG_G,C_BG_B);
    if(nhit<256){hits[nhit]=(Hit){login_btn_x,login_btn_y,login_btn_w,login_btn_h,505};nhit++;}

    /* Error message */
    if(login_error[0]) {
        text_center(f,W/2,login_btn_y+login_btn_h+10,login_error,1,C_RED_R,C_RED_G,C_RED_B);
    }

    /* Hint */
    text_center(f,W/2,by+bh+15,"Tap field to type  [Tab] Switch  [Enter] Login",1,C_DIM_R,C_DIM_G,C_DIM_B);
    text_center(f,W/2,by+bh+35,"Default: admin / admin123",1,C_DIM_R,C_DIM_G,C_DIM_B);

    /* -- Virtual Keyboard Overlay -- */
    if(kb_visible) {
        draw_keyboard(f, H - 210);
    }
}

/* ===== Draw: Paid Confirmation ===== */
static void draw_paid(FB *f) {
    int W=f->w, H=f->h;
    nhit=0;

    rect(f,0,0,W,H,C_BG_R,C_BG_G,C_BG_B);

    int cy=H/2;
    /* Big green check area */
    rect(f,W/2-120,cy-80,240,160,C_GRN_R/3,C_GRN_G/3,C_GRN_B/3);
    hline(f,W/2-120,cy-80,240,C_GRN_R,C_GRN_G,C_GRN_B);
    hline(f,W/2-120,cy+79,240,C_GRN_R,C_GRN_G,C_GRN_B);
    vline(f,W/2-120,cy-80,160,C_GRN_R,C_GRN_G,C_GRN_B);
    vline(f,W/2+119,cy-80,160,C_GRN_R,C_GRN_G,C_GRN_B);

    text_center(f,W/2,cy-40,"PAID",3,C_GRN_R,C_GRN_G,C_GRN_B);

    char buf[32];
    snprintf(buf,sizeof(buf),"Order #%04d",onum-1);
    text_center(f,W/2,cy+10,buf,2,C_GOLD_R,C_GOLD_G,C_GOLD_B);

    text_center(f,W/2,cy+50,pay_method==0?"CASH":"QRIS",2,pay_method==0?C_GRN_R:C_BLU_R, pay_method==0?C_GRN_G:C_BLU_G, pay_method==0?C_GRN_B:C_BLU_B);
}

/* ===== Draw: Menu Management ===== */
static void draw_menu_mgmt(FB *f) {
    int W=f->w, H=f->h;
    nhit=0;
    char buf[64];

    rect(f,0,0,W,H,C_BG_R,C_BG_G,C_BG_B);

    /* Header */
    rect(f,0,0,W,40,C_PNL_R,C_PNL_G,C_PNL_B);
    hline(f,0,39,W,C_BRD_R,C_BRD_G,C_BRD_B);
    text(f,100,12,"MENU MANAGEMENT",2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    draw_back_button(f, 2);

    if(mgmt_mode == 0) {
        /* List mode */

        /* --- CRUD touch button bar at top --- */
        {
            int bar_y = 48;
            int btn_h2 = 40, btn_gap = 8;
            int btn_w_add = 120, btn_w_edit = 100, btn_w_del = 120, btn_w_tog = 130;
            int total_bw = btn_w_add + btn_w_edit + btn_w_del + btn_w_tog + 3 * btn_gap;
            int bx = (W - total_bw) / 2;

            /* [+ ADD] - green/gold */
            rect(f,bx,bar_y,btn_w_add,btn_h2,C_GRN_R/2,C_GRN_G/2,C_GRN_B/2);
            hline(f,bx,bar_y,btn_w_add,C_GRN_R,C_GRN_G,C_GRN_B);
            hline(f,bx,bar_y+btn_h2-1,btn_w_add,C_GRN_R,C_GRN_G,C_GRN_B);
            vline(f,bx,bar_y,btn_h2,C_GRN_R,C_GRN_G,C_GRN_B);
            vline(f,bx+btn_w_add-1,bar_y,btn_h2,C_GRN_R,C_GRN_G,C_GRN_B);
            text_center(f,bx+btn_w_add/2,bar_y+(btn_h2-16)/2,"+ ADD",1,C_GRN_R,C_GRN_G,C_GRN_B);
            if(nhit<256){hits[nhit]=(Hit){bx,bar_y,btn_w_add,btn_h2,750};nhit++;}
            bx += btn_w_add + btn_gap;

            /* [EDIT] - gold */
            rect(f,bx,bar_y,btn_w_edit,btn_h2,C_GOLD_R/4,C_GOLD_G/4,C_GOLD_B/4);
            hline(f,bx,bar_y,btn_w_edit,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            hline(f,bx,bar_y+btn_h2-1,btn_w_edit,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            vline(f,bx,bar_y,btn_h2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            vline(f,bx+btn_w_edit-1,bar_y,btn_h2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            text_center(f,bx+btn_w_edit/2,bar_y+(btn_h2-16)/2,"EDIT",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            if(nhit<256){hits[nhit]=(Hit){bx,bar_y,btn_w_edit,btn_h2,751};nhit++;}
            bx += btn_w_edit + btn_gap;

            /* [DELETE] - red */
            rect(f,bx,bar_y,btn_w_del,btn_h2,C_RED_R/3,C_RED_G/3,C_RED_B/3);
            hline(f,bx,bar_y,btn_w_del,C_RED_R,C_RED_G,C_RED_B);
            hline(f,bx,bar_y+btn_h2-1,btn_w_del,C_RED_R,C_RED_G,C_RED_B);
            vline(f,bx,bar_y,btn_h2,C_RED_R,C_RED_G,C_RED_B);
            vline(f,bx+btn_w_del-1,bar_y,btn_h2,C_RED_R,C_RED_G,C_RED_B);
            text_center(f,bx+btn_w_del/2,bar_y+(btn_h2-16)/2,"DELETE",1,C_RED_R,C_RED_G,C_RED_B);
            if(nhit<256){hits[nhit]=(Hit){bx,bar_y,btn_w_del,btn_h2,752};nhit++;}
            bx += btn_w_del + btn_gap;

            /* [TOGGLE] - amber */
            rect(f,bx,bar_y,btn_w_tog,btn_h2,C_AMB_R/4,C_AMB_G/4,C_AMB_B/4);
            hline(f,bx,bar_y,btn_w_tog,C_AMB_R,C_AMB_G,C_AMB_B);
            hline(f,bx,bar_y+btn_h2-1,btn_w_tog,C_AMB_R,C_AMB_G,C_AMB_B);
            vline(f,bx,bar_y,btn_h2,C_AMB_R,C_AMB_G,C_AMB_B);
            vline(f,bx+btn_w_tog-1,bar_y,btn_h2,C_AMB_R,C_AMB_G,C_AMB_B);
            text_center(f,bx+btn_w_tog/2,bar_y+(btn_h2-16)/2,"TOGGLE",1,C_AMB_R,C_AMB_G,C_AMB_B);
            if(nhit<256){hits[nhit]=(Hit){bx,bar_y,btn_w_tog,btn_h2,753};nhit++;}
        }

        int y = 96;
        /* Header row */
        text(f,10,y,"#",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,30,y,"Name",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,300,y,"Price",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,430,y,"Category",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,560,y,"Status",1,C_DIM_R,C_DIM_G,C_DIM_B);
        y += 20;
        hline(f,10,y,W-20,C_BRD_R,C_BRD_G,C_BRD_B);
        y += 5;

        int row_h = 24;
        for(int i=0;i<menu_count&&y<H-40;i++){
            if(i==mgmt_sel) {
                rect(f,5,y-2,W-10,row_h,C_HI_R,C_HI_G,C_HI_B);
                text(f,10,y,">",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            }
            snprintf(buf,sizeof(buf),"%d",i+1);
            text(f,22,y,buf,1,C_DIM_R,C_DIM_G,C_DIM_B);
            text(f,30,y,menu_items[i].name,1, i==mgmt_sel?C_GOLD_R:C_TXT_R, i==mgmt_sel?C_GOLD_G:C_TXT_G, i==mgmt_sel?C_GOLD_B:C_TXT_B);
            fmt_rp(buf,sizeof(buf),menu_items[i].price);
            text(f,300,y,buf,1,C_AMB_R,C_AMB_G,C_AMB_B);
            const char *catname = menu_items[i].cat==0?"Drinks":(menu_items[i].cat==1?"Food":"Snacks");
            text(f,430,y,catname,1,C_DIM_R,C_DIM_G,C_DIM_B);
            text(f,560,y,menu_items[i].active?"ACTIVE":"INACTIVE",1, menu_items[i].active?C_GRN_R:C_RED_R, menu_items[i].active?C_GRN_G:C_RED_G, menu_items[i].active?C_GRN_B:C_RED_B);
            /* Hit region for row tap-to-select */
            if(nhit<256){hits[nhit]=(Hit){5,y-2,W-10,row_h,770+i};nhit++;}
            y += row_h + 2;
        }
    } else {
        /* Add/Edit form */
        const char *title = mgmt_mode==1 ? "ADD MENU ITEM" : "EDIT MENU ITEM";
        text_center(f,W/2,60,title,2,C_GOLD_R,C_GOLD_G,C_GOLD_B);

        int bx=W/2-220, by=100, bw=440, bh_form=290;
        rect(f,bx,by,bw,bh_form,C_PNL_R,C_PNL_G,C_PNL_B);
        hline(f,bx,by,bw,C_BRD_R,C_BRD_G,C_BRD_B);
        hline(f,bx,by+bh_form-1,bw,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,bx,by,bh_form,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,bx+bw-1,by,bh_form,C_BRD_R,C_BRD_G,C_BRD_B);

        int fy=by+15;
        /* Name */
        text(f,bx+20,fy,"Name:",1, mgmt_field==0?C_GOLD_R:C_DIM_R, mgmt_field==0?C_GOLD_G:C_DIM_G, mgmt_field==0?C_GOLD_B:C_DIM_B);
        fy+=18;
        int fx=bx+20,fw=bw-40,fh=28;
        rect(f,fx,fy,fw,fh, mgmt_field==0?C_SEL_R:C_BG_R, mgmt_field==0?C_SEL_G:C_BG_G, mgmt_field==0?C_SEL_B:C_BG_B);
        hline(f,fx,fy,fw, mgmt_field==0?C_GOLD_R:C_BRD_R, mgmt_field==0?C_GOLD_G:C_BRD_G, mgmt_field==0?C_GOLD_B:C_BRD_B);
        hline(f,fx,fy+fh-1,fw, mgmt_field==0?C_GOLD_R:C_BRD_R, mgmt_field==0?C_GOLD_G:C_BRD_G, mgmt_field==0?C_GOLD_B:C_BRD_B);
        text(f,fx+4,fy+6,mgmt_name,1,C_TXT_R,C_TXT_G,C_TXT_B);
        if(mgmt_field==0){int cx2=fx+4+tw(mgmt_name,1);rect(f,cx2,fy+4,2,fh-8,C_GOLD_R,C_GOLD_G,C_GOLD_B);}
        int name_fy=fy, name_fh=fh; /* save for hit region */

        /* Price */
        fy+=fh+12;
        text(f,bx+20,fy,"Price (Rp):",1, mgmt_field==1?C_GOLD_R:C_DIM_R, mgmt_field==1?C_GOLD_G:C_DIM_G, mgmt_field==1?C_GOLD_B:C_DIM_B);
        fy+=18;
        rect(f,fx,fy,fw,fh, mgmt_field==1?C_SEL_R:C_BG_R, mgmt_field==1?C_SEL_G:C_BG_G, mgmt_field==1?C_SEL_B:C_BG_B);
        hline(f,fx,fy,fw, mgmt_field==1?C_GOLD_R:C_BRD_R, mgmt_field==1?C_GOLD_G:C_BRD_G, mgmt_field==1?C_GOLD_B:C_BRD_B);
        hline(f,fx,fy+fh-1,fw, mgmt_field==1?C_GOLD_R:C_BRD_R, mgmt_field==1?C_GOLD_G:C_BRD_G, mgmt_field==1?C_GOLD_B:C_BRD_B);
        text(f,fx+4,fy+6,mgmt_price,1,C_TXT_R,C_TXT_G,C_TXT_B);
        if(mgmt_field==1){int cx2=fx+4+tw(mgmt_price,1);rect(f,cx2,fy+4,2,fh-8,C_GOLD_R,C_GOLD_G,C_GOLD_B);}
        int price_fy=fy, price_fh=fh; /* save for hit region */

        /* Category - tappable buttons */
        fy+=fh+12;
        text(f,bx+20,fy,"Category:",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
        fy+=20;
        const char *catnames[]={"DRINKS","FOOD","SNACKS"};
        int cat_btn_w=110, cat_btn_h=36, cat_gap=10;
        int cat_total_w=3*cat_btn_w+2*cat_gap;
        int cat_start_x=bx+(bw-cat_total_w)/2;
        for(int i=0;i<3;i++){
            int cbx=cat_start_x+i*(cat_btn_w+cat_gap);
            if(mgmt_cat==i){
                /* Selected: gold filled background */
                rect(f,cbx,fy,cat_btn_w,cat_btn_h,C_GOLD_R/3,C_GOLD_G/3,C_GOLD_B/3);
                hline(f,cbx,fy,cat_btn_w,C_GOLD_R,C_GOLD_G,C_GOLD_B);
                hline(f,cbx,fy+cat_btn_h-1,cat_btn_w,C_GOLD_R,C_GOLD_G,C_GOLD_B);
                vline(f,cbx,fy,cat_btn_h,C_GOLD_R,C_GOLD_G,C_GOLD_B);
                vline(f,cbx+cat_btn_w-1,fy,cat_btn_h,C_GOLD_R,C_GOLD_G,C_GOLD_B);
                text_center(f,cbx+cat_btn_w/2,fy+(cat_btn_h-16)/2,catnames[i],1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            } else {
                /* Unselected: outline/dimmed */
                rect(f,cbx,fy,cat_btn_w,cat_btn_h,C_BG_R,C_BG_G,C_BG_B);
                hline(f,cbx,fy,cat_btn_w,C_BRD_R,C_BRD_G,C_BRD_B);
                hline(f,cbx,fy+cat_btn_h-1,cat_btn_w,C_BRD_R,C_BRD_G,C_BRD_B);
                vline(f,cbx,fy,cat_btn_h,C_BRD_R,C_BRD_G,C_BRD_B);
                vline(f,cbx+cat_btn_w-1,fy,cat_btn_h,C_BRD_R,C_BRD_G,C_BRD_B);
                text_center(f,cbx+cat_btn_w/2,fy+(cat_btn_h-16)/2,catnames[i],1,C_DIM_R,C_DIM_G,C_DIM_B);
            }
            /* Hit region: 790=Drinks, 791=Food, 792=Snacks */
            if(nhit<256){hits[nhit]=(Hit){cbx,fy,cat_btn_w,cat_btn_h,790+i};nhit++;}
        }

        /* SAVE button */
        fy+=cat_btn_h+16;
        {
            int save_w=160, save_h=36;
            int save_x=bx+(bw-save_w)/2;
            rect(f,save_x,fy,save_w,save_h,C_GRN_R/2,C_GRN_G/2,C_GRN_B/2);
            hline(f,save_x,fy,save_w,C_GRN_R,C_GRN_G,C_GRN_B);
            hline(f,save_x,fy+save_h-1,save_w,C_GRN_R,C_GRN_G,C_GRN_B);
            vline(f,save_x,fy,save_h,C_GRN_R,C_GRN_G,C_GRN_B);
            vline(f,save_x+save_w-1,fy,save_h,C_GRN_R,C_GRN_G,C_GRN_B);
            text_center(f,save_x+save_w/2,fy+(save_h-16)/2,"SAVE",1,C_GRN_R,C_GRN_G,C_GRN_B);
            if(nhit<256){hits[nhit]=(Hit){save_x,fy,save_w,save_h,795};nhit++;} /* 795=save */
        }

        /* Hit regions for form fields (for keyboard activation) */
        {
            if(nhit<256){hits[nhit]=(Hit){fx,name_fy,fw,name_fh,720};nhit++;} /* 720=mgmt name field */
            if(nhit<256){hits[nhit]=(Hit){fx,price_fy,fw,price_fh,721};nhit++;} /* 721=mgmt price field */
        }
    }

    /* -- Virtual Keyboard Overlay -- */
    if(kb_visible) {
        draw_keyboard(f, H - 210);
    }
}

/* ===== Draw: User Management ===== */
static void draw_user_mgmt(FB *f) {
    int W=f->w, H=f->h;
    nhit=0;
    char buf[64];

    rect(f,0,0,W,H,C_BG_R,C_BG_G,C_BG_B);

    rect(f,0,0,W,40,C_PNL_R,C_PNL_G,C_PNL_B);
    hline(f,0,39,W,C_BRD_R,C_BRD_G,C_BRD_B);
    text(f,100,12,"USER MANAGEMENT",2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    draw_back_button(f, 2);

    if(umgmt_mode == 0) {

        /* --- CRUD touch button bar at top --- */
        {
            int bar_y = 48;
            int btn_h2 = 40, btn_gap = 8;
            int btn_w_add = 150, btn_w_edit = 100, btn_w_del = 120;
            int total_bw = btn_w_add + btn_w_edit + btn_w_del + 2 * btn_gap;
            int bx = (W - total_bw) / 2;

            /* [+ ADD USER] - green/gold */
            rect(f,bx,bar_y,btn_w_add,btn_h2,C_GRN_R/2,C_GRN_G/2,C_GRN_B/2);
            hline(f,bx,bar_y,btn_w_add,C_GRN_R,C_GRN_G,C_GRN_B);
            hline(f,bx,bar_y+btn_h2-1,btn_w_add,C_GRN_R,C_GRN_G,C_GRN_B);
            vline(f,bx,bar_y,btn_h2,C_GRN_R,C_GRN_G,C_GRN_B);
            vline(f,bx+btn_w_add-1,bar_y,btn_h2,C_GRN_R,C_GRN_G,C_GRN_B);
            text_center(f,bx+btn_w_add/2,bar_y+(btn_h2-16)/2,"+ ADD USER",1,C_GRN_R,C_GRN_G,C_GRN_B);
            if(nhit<256){hits[nhit]=(Hit){bx,bar_y,btn_w_add,btn_h2,760};nhit++;}
            bx += btn_w_add + btn_gap;

            /* [EDIT] - gold */
            rect(f,bx,bar_y,btn_w_edit,btn_h2,C_GOLD_R/4,C_GOLD_G/4,C_GOLD_B/4);
            hline(f,bx,bar_y,btn_w_edit,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            hline(f,bx,bar_y+btn_h2-1,btn_w_edit,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            vline(f,bx,bar_y,btn_h2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            vline(f,bx+btn_w_edit-1,bar_y,btn_h2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            text_center(f,bx+btn_w_edit/2,bar_y+(btn_h2-16)/2,"EDIT",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            if(nhit<256){hits[nhit]=(Hit){bx,bar_y,btn_w_edit,btn_h2,761};nhit++;}
            bx += btn_w_edit + btn_gap;

            /* [DELETE] - red */
            rect(f,bx,bar_y,btn_w_del,btn_h2,C_RED_R/3,C_RED_G/3,C_RED_B/3);
            hline(f,bx,bar_y,btn_w_del,C_RED_R,C_RED_G,C_RED_B);
            hline(f,bx,bar_y+btn_h2-1,btn_w_del,C_RED_R,C_RED_G,C_RED_B);
            vline(f,bx,bar_y,btn_h2,C_RED_R,C_RED_G,C_RED_B);
            vline(f,bx+btn_w_del-1,bar_y,btn_h2,C_RED_R,C_RED_G,C_RED_B);
            text_center(f,bx+btn_w_del/2,bar_y+(btn_h2-16)/2,"DELETE",1,C_RED_R,C_RED_G,C_RED_B);
            if(nhit<256){hits[nhit]=(Hit){bx,bar_y,btn_w_del,btn_h2,762};nhit++;}
        }

        int y = 96;
        text(f,10,y,"#",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,30,y,"Username",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,300,y,"Role",1,C_DIM_R,C_DIM_G,C_DIM_B);
        y+=20;
        hline(f,10,y,W-20,C_BRD_R,C_BRD_G,C_BRD_B);
        y+=5;

        int row_h = 24;
        for(int i=0;i<user_count&&y<H-40;i++){
            if(i==umgmt_sel) {
                rect(f,5,y-2,W-10,row_h,C_HI_R,C_HI_G,C_HI_B);
                text(f,10,y,">",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            }
            snprintf(buf,sizeof(buf),"%d",i+1);
            text(f,22,y,buf,1,C_DIM_R,C_DIM_G,C_DIM_B);
            text(f,30,y,user_list[i].username,1, i==umgmt_sel?C_GOLD_R:C_TXT_R, i==umgmt_sel?C_GOLD_G:C_TXT_G, i==umgmt_sel?C_GOLD_B:C_TXT_B);
            text(f,300,y,user_list[i].role,1, strcmp(user_list[i].role,"admin")==0?C_GOLD_R:C_DIM_R, strcmp(user_list[i].role,"admin")==0?C_GOLD_G:C_DIM_G, strcmp(user_list[i].role,"admin")==0?C_GOLD_B:C_DIM_B);
            /* Hit region for row tap-to-select */
            if(nhit<256){hits[nhit]=(Hit){5,y-2,W-10,row_h,780+i};nhit++;}
            y += row_h + 2;
        }
    } else {
        const char *title = umgmt_mode==1 ? "ADD USER" : "EDIT USER";
        text_center(f,W/2,60,title,2,C_GOLD_R,C_GOLD_G,C_GOLD_B);

        int bx=W/2-200, by=100, bw=400;
        rect(f,bx,by,bw,200,C_PNL_R,C_PNL_G,C_PNL_B);
        hline(f,bx,by,bw,C_BRD_R,C_BRD_G,C_BRD_B);
        hline(f,bx,by+199,bw,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,bx,by,200,C_BRD_R,C_BRD_G,C_BRD_B);
        vline(f,bx+bw-1,by,200,C_BRD_R,C_BRD_G,C_BRD_B);

        int fy=by+15, fx=bx+20, fw=bw-40, fh=22;

        /* Username */
        text(f,bx+20,fy,"Username:",1, umgmt_field==0?C_GOLD_R:C_DIM_R, umgmt_field==0?C_GOLD_G:C_DIM_G, umgmt_field==0?C_GOLD_B:C_DIM_B);
        fy+=18;
        rect(f,fx,fy,fw,fh, umgmt_field==0?C_SEL_R:C_BG_R, umgmt_field==0?C_SEL_G:C_BG_G, umgmt_field==0?C_SEL_B:C_BG_B);
        hline(f,fx,fy,fw, umgmt_field==0?C_GOLD_R:C_BRD_R, umgmt_field==0?C_GOLD_G:C_BRD_G, umgmt_field==0?C_GOLD_B:C_BRD_B);
        hline(f,fx,fy+fh-1,fw, umgmt_field==0?C_GOLD_R:C_BRD_R, umgmt_field==0?C_GOLD_G:C_BRD_G, umgmt_field==0?C_GOLD_B:C_BRD_B);
        text(f,fx+4,fy+3,umgmt_uname,1,C_TXT_R,C_TXT_G,C_TXT_B);
        if(umgmt_field==0){int cx2=fx+4+tw(umgmt_uname,1);rect(f,cx2,fy+2,2,fh-4,C_GOLD_R,C_GOLD_G,C_GOLD_B);}

        /* Password */
        fy+=35;
        text(f,bx+20,fy,"Password:",1, umgmt_field==1?C_GOLD_R:C_DIM_R, umgmt_field==1?C_GOLD_G:C_DIM_G, umgmt_field==1?C_GOLD_B:C_DIM_B);
        if(umgmt_mode==2) text(f,bx+120,fy,"(blank=keep)",1,C_DIM_R,C_DIM_G,C_DIM_B);
        fy+=18;
        rect(f,fx,fy,fw,fh, umgmt_field==1?C_SEL_R:C_BG_R, umgmt_field==1?C_SEL_G:C_BG_G, umgmt_field==1?C_SEL_B:C_BG_B);
        hline(f,fx,fy,fw, umgmt_field==1?C_GOLD_R:C_BRD_R, umgmt_field==1?C_GOLD_G:C_BRD_G, umgmt_field==1?C_GOLD_B:C_BRD_B);
        hline(f,fx,fy+fh-1,fw, umgmt_field==1?C_GOLD_R:C_BRD_R, umgmt_field==1?C_GOLD_G:C_BRD_G, umgmt_field==1?C_GOLD_B:C_BRD_B);
        char masked[32]; int plen=strlen(umgmt_pass);
        for(int i=0;i<plen&&i<31;i++) masked[i]='*'; masked[plen]=0;
        text(f,fx+4,fy+3,masked,1,C_TXT_R,C_TXT_G,C_TXT_B);
        if(umgmt_field==1){int cx2=fx+4+tw(masked,1);rect(f,cx2,fy+2,2,fh-4,C_GOLD_R,C_GOLD_G,C_GOLD_B);}

        /* Role */
        fy+=35;
        text(f,bx+20,fy,"Role:",1, umgmt_field==2?C_GOLD_R:C_DIM_R, umgmt_field==2?C_GOLD_G:C_DIM_G, umgmt_field==2?C_GOLD_B:C_DIM_B);
        fy+=18;
        const char *roles[]={"admin","cashier"};
        for(int i=0;i<2;i++){
            int bx2=fx+i*120;
            if(umgmt_role==i){
                rect(f,bx2,fy,110,fh,C_GOLD_R/3,C_GOLD_G/3,C_GOLD_B/3);
                text_center(f,bx2+55,fy+3,roles[i],1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            } else {
                rect(f,bx2,fy,110,fh,C_BG_R,C_BG_G,C_BG_B);
                hline(f,bx2,fy,110,C_BRD_R,C_BRD_G,C_BRD_B);
                hline(f,bx2,fy+fh-1,110,C_BRD_R,C_BRD_G,C_BRD_B);
                text_center(f,bx2+55,fy+3,roles[i],1,C_DIM_R,C_DIM_G,C_DIM_B);
            }
        }

        text_center(f,W/2,by+175,"[Tab] Next  [L/R] Role  [Enter] Save",1,C_DIM_R,C_DIM_G,C_DIM_B);

        /* Hit regions for form fields */
        {
            int fy2=by+15+18, fx2=bx+20, fw2=bw-40, fh2=22;
            if(nhit<256){hits[nhit]=(Hit){fx2,fy2,fw2,fh2,730};nhit++;} /* 730=umgmt username field */
            fy2+=35+18;
            if(nhit<256){hits[nhit]=(Hit){fx2,fy2,fw2,fh2,731};nhit++;} /* 731=umgmt password field */
        }
    }

    /* -- Virtual Keyboard Overlay -- */
    if(kb_visible) {
        draw_keyboard(f, H - 210);
    }
}

/* ===== Draw: Order History ===== */
static void draw_order_history(FB *f) {
    int W=f->w, H=f->h;
    nhit=0;
    char buf[64];

    rect(f,0,0,W,H,C_BG_R,C_BG_G,C_BG_B);

    rect(f,0,0,W,40,C_PNL_R,C_PNL_G,C_PNL_B);
    hline(f,0,39,W,C_BRD_R,C_BRD_G,C_BRD_B);
    text(f,100,12,"ORDER HISTORY",2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
    draw_back_button(f, 2);

    if(hist_detail == 0) {
        text(f,10,50,"[Enter] View Detail  [Up/Dn] Navigate",1,C_DIM_R,C_DIM_G,C_DIM_B);

        int y = 70;
        text(f,10,y,"#",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,60,y,"Date",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,260,y,"Total",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,420,y,"Method",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,530,y,"Cashier",1,C_DIM_R,C_DIM_G,C_DIM_B);
        y+=20;
        hline(f,10,y,W-20,C_BRD_R,C_BRD_G,C_BRD_B);
        y+=5;

        if(hist_count==0) {
            text_center(f,W/2,y+40,"No orders yet",1,C_DIM_R,C_DIM_G,C_DIM_B);
        }

        for(int i=0;i<hist_count&&y<H-40;i++){
            if(i==hist_sel) {
                rect(f,5,y-2,W-10,20,C_HI_R,C_HI_G,C_HI_B);
                text(f,10,y,">",1,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            }
            snprintf(buf,sizeof(buf),"%04d",hist_orders[i].id);
            text(f,22,y,buf,1,C_DIM_R,C_DIM_G,C_DIM_B);
            text(f,60,y,hist_orders[i].date,1, i==hist_sel?C_GOLD_R:C_TXT_R, i==hist_sel?C_GOLD_G:C_TXT_G, i==hist_sel?C_GOLD_B:C_TXT_B);
            fmt_rp(buf,sizeof(buf),hist_orders[i].total);
            text(f,260,y,buf,1,C_AMB_R,C_AMB_G,C_AMB_B);
            text(f,420,y,hist_orders[i].method,1, strcmp(hist_orders[i].method,"CASH")==0?C_GRN_R:C_BLU_R, strcmp(hist_orders[i].method,"CASH")==0?C_GRN_G:C_BLU_G, strcmp(hist_orders[i].method,"CASH")==0?C_GRN_B:C_BLU_B);
            text(f,530,y,hist_orders[i].cashier,1,C_DIM_R,C_DIM_G,C_DIM_B);
            y+=22;
        }
    } else {
        /* Detail view */
        snprintf(buf,sizeof(buf),"ORDER #%04d DETAIL",hist_detail_id);
        text_center(f,W/2,50,buf,2,C_GOLD_R,C_GOLD_G,C_GOLD_B);

        /* Find the order in hist_orders for summary */
        int oidx=-1;
        for(int i=0;i<hist_count;i++) if(hist_orders[i].id==hist_detail_id){oidx=i;break;}

        int y=90;
        if(oidx>=0){
            snprintf(buf,sizeof(buf),"Date: %s",hist_orders[oidx].date);
            text(f,60,y,buf,1,C_TXT_R,C_TXT_G,C_TXT_B); y+=20;
            snprintf(buf,sizeof(buf),"Cashier: %s",hist_orders[oidx].cashier);
            text(f,60,y,buf,1,C_TXT_R,C_TXT_G,C_TXT_B); y+=20;
            snprintf(buf,sizeof(buf),"Payment: %s",hist_orders[oidx].method);
            text(f,60,y,buf,1,C_TXT_R,C_TXT_G,C_TXT_B); y+=20;
        }
        y+=5;
        hline(f,60,y,W-120,C_BRD_R,C_BRD_G,C_BRD_B);
        y+=10;

        text(f,60,y,"Item",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,350,y,"Qty",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,420,y,"Price",1,C_DIM_R,C_DIM_G,C_DIM_B);
        text(f,560,y,"Subtotal",1,C_DIM_R,C_DIM_G,C_DIM_B);
        y+=18;
        hline(f,60,y,W-120,C_BRD_R,C_BRD_G,C_BRD_B);
        y+=5;

        for(int i=0;i<hist_item_count;i++){
            text(f,60,y,hist_items[i].name,1,C_TXT_R,C_TXT_G,C_TXT_B);
            snprintf(buf,sizeof(buf),"%d",hist_items[i].qty);
            text(f,350,y,buf,1,C_TXT_R,C_TXT_G,C_TXT_B);
            fmt_rp(buf,sizeof(buf),hist_items[i].price);
            text(f,420,y,buf,1,C_AMB_R,C_AMB_G,C_AMB_B);
            fmt_rp(buf,sizeof(buf),hist_items[i].qty*hist_items[i].price);
            text(f,560,y,buf,1,C_AMB_R,C_AMB_G,C_AMB_B);
            y+=20;
        }

        y+=10;
        hline(f,60,y,W-120,C_BRD_R,C_BRD_G,C_BRD_B);
        y+=10;
        if(oidx>=0){
            fmt_rp(buf,sizeof(buf),hist_orders[oidx].total);
            text(f,60,y,"TOTAL:",2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
            text(f,300,y,buf,2,C_GOLD_R,C_GOLD_G,C_GOLD_B);
        }

        text_center(f,W/2,H-50,"[ESC] Back to list",1,C_DIM_R,C_DIM_G,C_DIM_B);
    }
}

/* ===== Cursor (pixel-art arrow, amber/gold) ===== */
static const unsigned char cursor_data[16] = {
    0x80,0xC0,0xE0,0xF0,0xF8,0xFC,0xFE,0xFF,0xFC,0xFC,0xCC,0x86,0x06,0x03,0x03,0x00,
};
static const unsigned char cursor_outline[16] = {
    0xC0,0xE0,0xF0,0xF8,0xFC,0xFE,0xFF,0xFF,0xFE,0xFE,0xEE,0xCF,0x8F,0x07,0x07,0x03,
};

static void draw_cursor(FB *f, int cx, int cy) {
    for(int row=0;row<16;row++){
        unsigned char outline=cursor_outline[row];
        unsigned char fill=cursor_data[row];
        for(int col=0;col<8;col++){
            int sx=cx+col*2, sy=cy+row*2;
            if(outline&(0x80>>col)){
                if(fill&(0x80>>col))
                    rect(f,sx,sy,2,2,212,175,55);
                else
                    rect(f,sx,sy,2,2,40,30,10);
            }
        }
    }
}

/* ===== Text Input Helper ===== */
/* Map evdev key code to ASCII character. Returns 0 if not a printable char. */
static char key_to_char(int code, int shift) {
    /* Letter keys */
    if(code >= KEY_Q && code <= KEY_P) {
        const char row[] = "qwertyuiop";
        char c = row[code - KEY_Q];
        return shift ? (c - 32) : c;
    }
    if(code >= KEY_A && code <= KEY_L) {
        const char row[] = "asdfghjkl";
        char c = row[code - KEY_A];
        return shift ? (c - 32) : c;
    }
    if(code >= KEY_Z && code <= KEY_M) {
        const char row[] = "zxcvbnm";
        char c = row[code - KEY_Z];
        return shift ? (c - 32) : c;
    }
    /* Number keys */
    if(code >= KEY_1 && code <= KEY_9) return '1' + (code - KEY_1);
    if(code == KEY_0) return '0';
    /* Special */
    if(code == KEY_SPACE) return ' ';
    if(code == KEY_DOT || code == KEY_KPDOT) return '.';
    if(code == KEY_MINUS || code == KEY_KPMINUS) return '-';
    if(code == KEY_EQUAL) return shift ? '+' : '=';
    if(code == KEY_SLASH) return '/';
    if(code == KEY_SEMICOLON) return ';';
    if(code == KEY_APOSTROPHE) return '\'';
    if(code == KEY_LEFTBRACE) return '[';
    if(code == KEY_RIGHTBRACE) return ']';
    if(code == KEY_COMMA) return ',';
    /* Numpad */
    if(code >= KEY_KP1 && code <= KEY_KP9) return '1' + (code - KEY_KP1);
    if(code == KEY_KP0) return '0';
    return 0;
}

/* Append char to text buffer */
static void text_input_char(char *buf, int maxlen, char c) {
    int len = strlen(buf);
    if(len < maxlen - 1) {
        buf[len] = c;
        buf[len+1] = 0;
    }
}

/* Backspace on text buffer */
static void text_input_backspace(char *buf) {
    int len = strlen(buf);
    if(len > 0) buf[len-1] = 0;
}

/* ===== Main ===== */
int main(void) {
    const char *dev=getenv("FBDEV");
    if(!dev) dev="/dev/fb0";
    int fd=open(dev,O_RDWR);
    if(fd<0){perror("open fb");return 1;}

    struct fb_var_screeninfo vi;
    struct fb_fix_screeninfo fi;
    ioctl(fd,FBIOGET_VSCREENINFO,&vi);
    ioctl(fd,FBIOGET_FSCREENINFO,&fi);

    FB fb={.w=vi.xres,.h=vi.yres,.bpp=vi.bits_per_pixel/8,.stride=fi.line_length,.size=fi.line_length*vi.yres};
    unsigned char *fbmem=mmap(0,fb.size,PROT_READ|PROT_WRITE,MAP_SHARED,fd,0);
    if(fbmem==MAP_FAILED){perror("mmap");return 1;}

    unsigned char *back=malloc(fb.size);
    if(!back){perror("malloc");return 1;}
    fb.mem=back;

    /* Initialize database */
    if(db_init() < 0) {
        fprintf(stderr, "DB init failed, using defaults\n");
        load_default_menu();
    }
#ifdef SQLITE_INTEGRATION
    db_load_menu();
#endif
    if(menu_count == 0) load_default_menu();

    /* Suppress console text rendering (KD_GRAPHICS) */
    int tty_fd=-1;
    for(int i=0;i<4;i++){
        char path[16]; snprintf(path,sizeof(path),"/dev/tty%d",i);
        tty_fd=open(path,O_RDWR);
        if(tty_fd>=0) break;
    }
    if(tty_fd<0) tty_fd=open("/dev/console",O_RDWR);
    if(tty_fd>=0){
        ioctl(tty_fd,KDSETMODE,KD_GRAPHICS);
        fprintf(stderr,"KD_GRAPHICS set on tty fd=%d\n",tty_fd);
    }

    /* Open input devices */
    int inp[12]; int ninp=0;
    for(int i=0;i<8;i++){
        char path[32];
        snprintf(path,sizeof(path),"/dev/input/event%d",i);
        int ifd=open(path,O_RDONLY|O_NONBLOCK);
        if(ifd>=0){
            ioctl(ifd,EVIOCGRAB,1);
            inp[ninp++]=ifd;
            fprintf(stderr,"Input: %s fd=%d (grabbed)\n",path,ifd);
        }
    }
    {
        int mfd=open("/dev/input/mice",O_RDONLY|O_NONBLOCK);
        if(mfd>=0){inp[ninp++]=mfd;fprintf(stderr,"Input: /dev/input/mice fd=%d\n",mfd);}
    }
    fprintf(stderr,"WayangOS POS v3.1: %dx%d, %d bpp, %d inputs, tty=%d\n",fb.w,fb.h,fb.bpp*8,ninp,tty_fd);

    int mx=fb.w/2, my=fb.h/2;
    int redraw=1, last_min=-1;
    int shift_held=0;

    /* Clean buffer for fast cursor updates */
    unsigned char *clean=malloc(fb.size);
    if(!clean){perror("malloc clean");return 1;}
    int omx=-1,omy=-1;
    #define CUR_W 16
    #define CUR_H 32
    int cursor_dirty=0;

    app_state = STATE_WELCOME;

    /* ===== WELCOME SCREEN ===== */
welcome:
    app_state = STATE_WELCOME;
    {
        rect(&fb,0,0,fb.w,fb.h,15,12,8);
        rect(&fb,0,0,fb.w,3,212,175,55);
        int cy=fb.h/2;
        text_center(&fb,fb.w/2,cy-90,"W A Y A N G O S",3,212,175,55);
        int sepw=300;
        rect(&fb,fb.w/2-sepw/2,cy-45,sepw,2,120,100,40);
        text_center(&fb,fb.w/2,cy-25,"Point of Sale System",1,180,150,60);
        text_center(&fb,fb.w/2,cy-5,"Warung Kopi Nusantara",1,140,120,50);
        text_center(&fb,fb.w/2,cy+25,"v3.1.0",1,100,85,35);
#ifdef SQLITE_INTEGRATION
        text_center(&fb,fb.w/2,cy+40,"+ SQLite",1,80,70,30);
#endif
        rect(&fb,fb.w/2-sepw/2,cy+55,sepw,2,120,100,40);
        rect(&fb,fb.w/2-140,cy+70,280,36,35,28,15);
        rect(&fb,fb.w/2-139,cy+71,278,34,45,35,18);
        text_center(&fb,fb.w/2,cy+80,"Tap or press ENTER",1,212,175,55);
        text_center(&fb,fb.w/2,fb.h-30,"Powered by WayangOS - Ultra Minimal Linux",1,80,65,30);
        text_center(&fb,fb.w/2,fb.h-50,"[ESC] Exit to shell",1,60,50,25);
        rect(&fb,0,fb.h-3,fb.w,3,212,175,55);
        memcpy(fbmem,back,fb.size);

        int waiting=1;
        while(waiting){
            fd_set rfds; FD_ZERO(&rfds);
            int maxfd=0;
            for(int i=0;i<ninp;i++){FD_SET(inp[i],&rfds);if(inp[i]>maxfd)maxfd=inp[i];}
            struct timeval tv={1,0};
            int r=select(maxfd+1,&rfds,NULL,NULL,&tv);
            if(r<=0) continue;
            for(int i=0;i<ninp;i++){
                if(!FD_ISSET(inp[i],&rfds)) continue;
                struct input_event ev;
                while(read(inp[i],&ev,sizeof(ev))==sizeof(ev)){
                    if(ev.type==EV_KEY&&ev.value==1){
                        if(ev.code==KEY_ENTER||ev.code==KEY_KPENTER||ev.code==KEY_SPACE)
                            waiting=0;
                        else if(ev.code==BTN_LEFT||ev.code==BTN_TOUCH)
                            waiting=0;  /* tap anywhere to start */
                        else if(ev.code==KEY_ESC) goto done;
                    }
                }
            }
        }
    }

    /* Go to login screen */
    login_user[0]=0; login_pass[0]=0; login_field=0; login_error[0]=0;
    app_state = STATE_LOGIN;
    redraw=1; last_min=-1;

    /* ===== Main Event Loop ===== */
    while(1){
        if(redraw){
            switch(app_state) {
                case STATE_LOGIN:    draw_login(&fb); break;
                case STATE_POS:      draw_pos(&fb); break;
                case STATE_MENU_MGMT: draw_menu_mgmt(&fb); break;
                case STATE_USER_MGMT: draw_user_mgmt(&fb); break;
                case STATE_ORDER_HISTORY: draw_order_history(&fb); break;
                case STATE_PAID_CONFIRM: draw_paid(&fb); break;
                default: draw_pos(&fb); break;
            }
            memcpy(clean,back,fb.size);
            draw_cursor(&fb,mx,my);
            memcpy(fbmem,back,fb.size);
            omx=mx; omy=my;
            redraw=0; cursor_dirty=0;
        } else if(cursor_dirty){
            if(omx>=0){
                for(int row=0;row<CUR_H&&omy+row<fb.h;row++){
                    int y=omy+row;
                    long off=y*fb.stride+omx*fb.bpp;
                    int w=CUR_W*fb.bpp;
                    if(omx+CUR_W>fb.w) w=(fb.w-omx)*fb.bpp;
                    if(w>0) memcpy(fbmem+off,clean+off,w);
                }
            }
            for(int row=0;row<CUR_H&&my+row<fb.h;row++){
                int y=my+row;
                long off=y*fb.stride+mx*fb.bpp;
                int w=CUR_W*fb.bpp;
                if(mx+CUR_W>fb.w) w=(fb.w-mx)*fb.bpp;
                if(w>0){memcpy(back+off,clean+off,w);memcpy(fbmem+off,clean+off,w);}
            }
            draw_cursor(&fb,mx,my);
            for(int row=0;row<CUR_H&&my+row<fb.h;row++){
                int y=my+row;
                long off=y*fb.stride+mx*fb.bpp;
                int w=CUR_W*fb.bpp;
                if(mx+CUR_W>fb.w) w=(fb.w-mx)*fb.bpp;
                if(w>0) memcpy(fbmem+off,back+off,w);
            }
            omx=mx; omy=my;
            cursor_dirty=0;
        }

        /* Check paid confirmation timeout */
        if(app_state == STATE_PAID_CONFIRM && time(NULL) - paid_time >= 2) {
            app_state = STATE_POS;
            redraw = 1;
            continue;
        }

        fd_set rfds; FD_ZERO(&rfds);
        int maxfd=0;
        for(int i=0;i<ninp;i++){FD_SET(inp[i],&rfds);if(inp[i]>maxfd)maxfd=inp[i];}

        struct timeval tv;
        if(app_state == STATE_PAID_CONFIRM) {
            tv.tv_sec=0; tv.tv_usec=200000; /* Fast poll during paid animation */
        } else {
            tv.tv_sec=5; tv.tv_usec=0;
        }
        int r=select(maxfd+1,&rfds,NULL,NULL,&tv);

        if(r==0){
            if(app_state == STATE_PAID_CONFIRM) { redraw=1; continue; }
            time_t now=time(NULL);struct tm *t=localtime(&now);
            if(t->tm_min!=last_min){last_min=t->tm_min;redraw=1;}
            continue;
        }

        /* Read input events */
        for(int i=0;i<ninp;i++){
            if(!FD_ISSET(inp[i],&rfds)) continue;
            struct input_event ev;
            while(read(inp[i],&ev,sizeof(ev))==sizeof(ev)){

                /* Track shift state */
                if(ev.type==EV_KEY && (ev.code==KEY_LEFTSHIFT||ev.code==KEY_RIGHTSHIFT)) {
                    shift_held = (ev.value != 0);
                }

                if(ev.type==EV_KEY&&ev.value==1){

                    /* ===== VIRTUAL KEYBOARD TOUCH HANDLING ===== */
                    if(kb_visible && (ev.code==BTN_LEFT||ev.code==BTN_TOUCH)) {
                        int a=hit_test(mx,my);
                        if(a >= 600 && a <= 644) {
                            /* Keyboard key pressed */
                            if(a >= 600 && a <= 608) {
                                /* Digits 1-9 */
                                kb_type_char('1' + (a - 600));
                            } else if(a == 609) {
                                /* Digit 0 */
                                kb_type_char('0');
                            } else if(a >= 610 && a <= 619) {
                                /* Row 2: qwertyuiop */
                                const char *row = kb_shift ? "QWERTYUIOP" : "qwertyuiop";
                                kb_type_char(row[a - 610]);
                            } else if(a >= 620 && a <= 628) {
                                /* Row 3: asdfghjkl */
                                const char *row = kb_shift ? "ASDFGHJKL" : "asdfghjkl";
                                kb_type_char(row[a - 620]);
                            } else if(a >= 629 && a <= 635) {
                                /* Row 4: zxcvbnm */
                                const char *row = kb_shift ? "ZXCVBNM" : "zxcvbnm";
                                kb_type_char(row[a - 629]);
                            } else if(a == 640) {
                                /* SPACE */
                                kb_type_char(' ');
                            } else if(a == 641) {
                                /* BACKSPACE */
                                kb_backspace();
                            } else if(a == 642) {
                                /* ENTER - submit and hide keyboard */
                                /* Context-dependent behavior */
                                if(app_state == STATE_LOGIN) {
                                    if(login_field == 0) {
                                        /* Switch to password field */
                                        login_field = 1;
                                        kb_show(login_pass, 31, 1, 1);
                                    } else {
                                        /* Submit login */
                                        kb_hide();
                                        if(db_authenticate(login_user, login_pass)) {
                                            app_state = STATE_POS;
                                            order_clear();
#ifdef SQLITE_INTEGRATION
                                            db_load_menu();
#endif
                                            login_error[0]=0;
                                        } else {
                                            strcpy(login_error, "Invalid username or password");
                                        }
                                    }
                                } else if(app_state == STATE_MENU_MGMT && mgmt_mode > 0) {
                                    if(mgmt_field == 0) {
                                        /* Move to price field */
                                        mgmt_field = 1;
                                        kb_show(mgmt_price, 15, 3, 0);
                                    } else {
                                        /* Submit form */
                                        kb_hide();
                                        int price=atoi(mgmt_price);
                                        if(mgmt_name[0]&&price>0){
                                            if(mgmt_mode==1) db_add_menu_item(mgmt_name,price,mgmt_cat);
                                            else db_update_menu_item(mgmt_edit_id,mgmt_name,price,mgmt_cat);
#ifdef SQLITE_INTEGRATION
                                            db_load_menu();
#endif
                                            mgmt_mode=0;
                                        }
                                    }
                                } else if(app_state == STATE_USER_MGMT && umgmt_mode > 0) {
                                    if(umgmt_field == 0) {
                                        umgmt_field = 1;
                                        kb_show(umgmt_pass, 31, 5, 1);
                                    } else {
                                        kb_hide();
                                        const char *role=umgmt_role==0?"admin":"cashier";
                                        if(umgmt_uname[0]){
                                            if(umgmt_mode==1 && umgmt_pass[0]) {
                                                db_add_user(umgmt_uname,umgmt_pass,role);
                                            } else if(umgmt_mode==2) {
                                                db_update_user(umgmt_edit_id,umgmt_uname,umgmt_pass,role);
                                            }
                                            db_load_users();
                                            umgmt_mode=0;
                                        }
                                    }
                                } else {
                                    kb_hide();
                                }
                            } else if(a == 643) {
                                /* SHIFT toggle */
                                kb_shift = !kb_shift;
                            } else if(a == 644) {
                                /* Close keyboard */
                                kb_hide();
                            }
                            redraw = 1;
                            continue;  /* consume the event */
                        }
                        /* If tap was outside keyboard area but keyboard is visible,
                           let the state handler process it (might be a field tap) */
                    }

                    /* ===== NAV BUTTON HANDLING (POS state) ===== */
                    if(app_state == STATE_POS && (ev.code==BTN_LEFT||ev.code==BTN_TOUCH)) {
                        int a=hit_test(mx,my);
                        if(a >= 700 && a <= 704) {
                            kb_hide();
                            if(a == 700) {
                                /* MENU management */
#ifdef SQLITE_INTEGRATION
                                db_load_menu();
#endif
                                mgmt_sel=0; mgmt_mode=0;
                                app_state=STATE_MENU_MGMT;
                            } else if(a == 701) {
                                /* USERS management (admin only) */
                                if(strcmp(current_role,"admin")==0) {
                                    db_load_users();
                                    umgmt_sel=0; umgmt_mode=0;
                                    app_state=STATE_USER_MGMT;
                                }
                            } else if(a == 702) {
                                /* HISTORY */
                                db_load_order_history();
                                hist_sel=0; hist_detail=0;
                                app_state=STATE_ORDER_HISTORY;
                            } else if(a == 703) {
                                /* HELP */
                                show_help=!show_help;
                            } else if(a == 704) {
                                /* LOGOUT */
                                current_user_id=0; current_username[0]=0; current_role[0]=0;
                                login_user[0]=0; login_pass[0]=0; login_field=0; login_error[0]=0;
                                app_state=STATE_LOGIN;
                            }
                            redraw=1;
                            continue;
                        }
                    }

                    /* ===== BACK BUTTON HANDLING (sub-pages) ===== */
                    if((app_state==STATE_MENU_MGMT||app_state==STATE_USER_MGMT||app_state==STATE_ORDER_HISTORY)
                       && (ev.code==BTN_LEFT||ev.code==BTN_TOUCH)) {
                        int a=hit_test(mx,my);
                        if(a == 710) {
                            kb_hide();
                            if(app_state==STATE_MENU_MGMT) {
#ifdef SQLITE_INTEGRATION
                                db_load_menu();
#endif
                            }
                            if(app_state==STATE_ORDER_HISTORY && hist_detail) {
                                hist_detail=0;
                            } else {
                                app_state=STATE_POS;
                            }
                            redraw=1;
                            continue;
                        }
                        /* Form field taps for keyboard activation */
                        if(a == 720 && app_state==STATE_MENU_MGMT && mgmt_mode > 0) {
                            mgmt_field = 0;
                            kb_show(mgmt_name, 31, 2, 0);
                            redraw=1; continue;
                        }
                        if(a == 721 && app_state==STATE_MENU_MGMT && mgmt_mode > 0) {
                            mgmt_field = 1;
                            kb_show(mgmt_price, 15, 3, 0);
                            redraw=1; continue;
                        }
                        /* Category selection buttons (touchscreen) */
                        if(a >= 790 && a <= 792 && app_state==STATE_MENU_MGMT && mgmt_mode > 0) {
                            mgmt_cat = a - 790;
                            mgmt_field = 2; /* visually select category field */
                            kb_hide(); /* category doesn't need keyboard */
                            redraw=1; continue;
                        }
                        /* SAVE button (touchscreen) */
                        if(a == 795 && app_state==STATE_MENU_MGMT && mgmt_mode > 0) {
                            int price=atoi(mgmt_price);
                            if(mgmt_name[0]&&price>0){
                                if(mgmt_mode==1) db_add_menu_item(mgmt_name,price,mgmt_cat);
                                else db_update_menu_item(mgmt_edit_id,mgmt_name,price,mgmt_cat);
#ifdef SQLITE_INTEGRATION
                                db_load_menu();
#endif
                                kb_hide();
                                mgmt_mode=0;
                            }
                            redraw=1; continue;
                        }
                        if(a == 730 && app_state==STATE_USER_MGMT && umgmt_mode > 0) {
                            umgmt_field = 0;
                            kb_show(umgmt_uname, 31, 4, 0);
                            redraw=1; continue;
                        }
                        if(a == 731 && app_state==STATE_USER_MGMT && umgmt_mode > 0) {
                            umgmt_field = 1;
                            kb_show(umgmt_pass, 31, 5, 1);
                            redraw=1; continue;
                        }
                    }

                    /* ===== LOGIN STATE ===== */
                    if(app_state == STATE_LOGIN) {
                        if(ev.code==KEY_ESC){ kb_hide(); goto welcome; }
                        else if(ev.code==KEY_TAB) { login_field = 1 - login_field; redraw=1; }
                        else if(ev.code==BTN_LEFT||ev.code==BTN_TOUCH) {
                            int a=hit_test(mx,my);
                            if(a==506) { login_field=0; kb_show(login_user, 31, 0, 0); redraw=1; }
                            else if(a==507) { login_field=1; kb_show(login_pass, 31, 1, 1); redraw=1; }
                            else if(a==505) {
                                /* LOGIN button tapped */
                                if(db_authenticate(login_user, login_pass)) {
                                    app_state = STATE_POS;
                                    order_clear();
#ifdef SQLITE_INTEGRATION
                                    db_load_menu();
#endif
                                    login_error[0]=0;
                                } else {
                                    strcpy(login_error, "Invalid username or password");
                                }
                                redraw=1;
                            }
                        }
                        else if(ev.code==KEY_ENTER||ev.code==KEY_KPENTER) {
                            if(db_authenticate(login_user, login_pass)) {
                                app_state = STATE_POS;
                                order_clear();
#ifdef SQLITE_INTEGRATION
                                db_load_menu();
#endif
                                login_error[0]=0;
                            } else {
                                strcpy(login_error, "Invalid username or password");
                            }
                            redraw=1;
                        }
                        else if(ev.code==KEY_BACKSPACE) {
                            if(login_field==0) text_input_backspace(login_user);
                            else text_input_backspace(login_pass);
                            redraw=1;
                        }
                        else {
                            char c = key_to_char(ev.code, shift_held);
                            if(c) {
                                if(login_field==0) text_input_char(login_user, 31, c);
                                else text_input_char(login_pass, 31, c);
                                redraw=1;
                            }
                        }
                    }

                    /* ===== POS STATE ===== */
                    else if(app_state == STATE_POS) {
                        /* F1 Help overlay toggle */
                        if(ev.code==KEY_F1){show_help=!show_help;redraw=1;}
                        /* If help is showing, ESC or F1 dismisses it */
                        else if(show_help && ev.code==KEY_ESC){show_help=0;redraw=1;}
                        else if(show_help) { /* ignore other keys while help is open */ }
                        else {
                        /* Get visible item list for mapping 1-9 keys */
                        int vis[64], nvis=0;
                        for(int mi=0;mi<menu_count;mi++)
                            if(menu_items[mi].active && (cur_cat==0||menu_items[mi].cat==cur_cat-1))
                                vis[nvis++]=mi;

                        /* Pagination: keys 1-9 map to CURRENT page */
                        int items_pp = 9;
                        int total_pg = (nvis + items_pp - 1) / items_pp;
                        if(total_pg < 1) total_pg = 1;

                        if(ev.code>=KEY_1&&ev.code<=KEY_9){
                            int v=ev.code-KEY_1;
                            int idx = menu_page * items_pp + v;
                            if(idx<nvis){order_add(vis[idx]);redraw=1;}
                        }
                        else if(ev.code>=KEY_KP1&&ev.code<=KEY_KP9){
                            int v=ev.code-KEY_KP1;
                            int idx = menu_page * items_pp + v;
                            if(idx<nvis){order_add(vis[idx]);redraw=1;}
                        }
                        /* Clear order: 0 or numpad 0 */
                        else if(ev.code==KEY_0||ev.code==KEY_KP0){order_clear();redraw=1;}
                        /* UP/DOWN to navigate order items */
                        else if(ev.code==KEY_UP){
                            if(ocnt>0){order_sel--;if(order_sel<0)order_sel=ocnt-1;redraw=1;}
                        }
                        else if(ev.code==KEY_DOWN){
                            if(ocnt>0){order_sel++;if(order_sel>=ocnt)order_sel=0;redraw=1;}
                        }
                        /* LEFT/RIGHT for page navigation */
                        else if(ev.code==KEY_LEFT||ev.code==KEY_PAGEUP){
                            if(menu_page>0){menu_page--;redraw=1;}
                        }
                        else if(ev.code==KEY_RIGHT||ev.code==KEY_PAGEDOWN){
                            if(menu_page<total_pg-1){menu_page++;redraw=1;}
                        }
                        /* Pay: Enter */
                        else if(ev.code==KEY_ENTER||ev.code==KEY_KPENTER){
                            if(ocnt>0){
                                db_save_order();
                                paid_time = time(NULL);
                                app_state = STATE_PAID_CONFIRM;
                                order_clear();
                            }
                            redraw=1;
                        }
                        /* Decrease qty of SELECTED item: minus or numpad minus */
                        else if(ev.code==KEY_MINUS||ev.code==KEY_KPMINUS){
                            if(ocnt>0){order_dec_selected();redraw=1;}
                        }
                        /* Delete SELECTED item: Backspace or Delete */
                        else if(ev.code==KEY_BACKSPACE||ev.code==KEY_DELETE){
                            if(ocnt>0){order_remove_selected();redraw=1;}
                        }
                        /* Payment method: C=Cash, R=QRIS */
                        else if(ev.code==KEY_C){pay_method=0;redraw=1;}
                        else if(ev.code==KEY_R){pay_method=1;redraw=1;}
                        /* Category tab - reset page on change */
                        else if(ev.code==KEY_TAB){cur_cat=(cur_cat+1)%4;menu_page=0;redraw=1;}
                        /* Menu Management */
                        else if(ev.code==KEY_M){
#ifdef SQLITE_INTEGRATION
                            db_load_menu();
#endif
                            mgmt_sel=0; mgmt_mode=0;
                            app_state=STATE_MENU_MGMT;
                            redraw=1;
                        }
                        /* User Management (admin only) */
                        else if(ev.code==KEY_U){
                            if(strcmp(current_role,"admin")==0){
                                db_load_users();
                                umgmt_sel=0; umgmt_mode=0;
                                app_state=STATE_USER_MGMT;
                                redraw=1;
                            }
                        }
                        /* Order History */
                        else if(ev.code==KEY_H){
                            db_load_order_history();
                            hist_sel=0; hist_detail=0;
                            app_state=STATE_ORDER_HISTORY;
                            redraw=1;
                        }
                        /* Logout */
                        else if(ev.code==KEY_L){
                            current_user_id=0; current_username[0]=0; current_role[0]=0;
                            login_user[0]=0; login_pass[0]=0; login_field=0; login_error[0]=0;
                            app_state=STATE_LOGIN;
                            redraw=1;
                        }
                        else if(ev.code==KEY_ESC) goto done;
                        /* Mouse/touch click */
                        else if(ev.code==BTN_LEFT||ev.code==BTN_TOUCH){
                            int a=hit_test(mx,my);
                            if(a>=0&&a<64){order_add(a);redraw=1;}
                            else if(a>=100&&a<=103){cur_cat=a-100;menu_page=0;redraw=1;}
                            else if(a==200){pay_method=0;redraw=1;}
                            else if(a==201){pay_method=1;redraw=1;}
                            else if(a==202){
                                if(ocnt>0){db_save_order();paid_time=time(NULL);app_state=STATE_PAID_CONFIRM;order_clear();}
                                redraw=1;
                            }
                            /* [+] buttons on order items */
                            else if(a>=300&&a<332){order_inc_at(a-300);redraw=1;}
                            /* [-] buttons on order items */
                            else if(a>=400&&a<432){order_dec_at(a-400);redraw=1;}
                            /* CLEAR ORDER button */
                            else if(a==500){if(ocnt>0){order_clear();redraw=1;}}
                            /* Page left arrow */
                            else if(a==501){if(menu_page>0){menu_page--;redraw=1;}}
                            /* Page right arrow */
                            else if(a==502){if(menu_page<total_pg-1){menu_page++;redraw=1;}}
                            /* Order scroll up */
                            else if(a==503){if(order_scroll>0){order_scroll--;redraw=1;}}
                            /* Order scroll down */
                            else if(a==504){order_scroll++;redraw=1;}
                        }
                        } /* end of non-help block */
                    }

                    /* ===== MENU MANAGEMENT STATE ===== */
                    else if(app_state == STATE_MENU_MGMT) {
                        if(mgmt_mode == 0) {
                            /* List mode */
                            if(ev.code==KEY_ESC){
#ifdef SQLITE_INTEGRATION
                                db_load_menu();
#endif
                                app_state=STATE_POS;redraw=1;
                            }
                            else if(ev.code==KEY_UP){if(menu_count>0){mgmt_sel--;if(mgmt_sel<0)mgmt_sel=menu_count-1;}redraw=1;}
                            else if(ev.code==KEY_DOWN){if(menu_count>0){mgmt_sel++;if(mgmt_sel>=menu_count)mgmt_sel=0;}redraw=1;}
                            else if(ev.code==KEY_A){
                                /* Add new */
                                mgmt_mode=1; mgmt_field=0;
                                mgmt_name[0]=0; mgmt_price[0]=0; mgmt_cat=0;
                                kb_show(mgmt_name, 31, 2, 0);
                                redraw=1;
                            }
                            else if(ev.code==KEY_ENTER||ev.code==KEY_KPENTER){
                                /* Edit selected */
                                if(mgmt_sel>=0&&mgmt_sel<menu_count){
                                    mgmt_mode=2; mgmt_field=0;
                                    mgmt_edit_id=menu_items[mgmt_sel].id;
                                    strncpy(mgmt_name,menu_items[mgmt_sel].name,31);
                                    snprintf(mgmt_price,sizeof(mgmt_price),"%d",menu_items[mgmt_sel].price);
                                    mgmt_cat=menu_items[mgmt_sel].cat;
                                    kb_show(mgmt_name, 31, 2, 0);
                                    redraw=1;
                                }
                            }
                            else if(ev.code==KEY_D){
                                /* Delete selected */
                                if(mgmt_sel>=0&&mgmt_sel<menu_count){
                                    db_delete_menu_item(menu_items[mgmt_sel].id);
#ifdef SQLITE_INTEGRATION
                                    db_load_menu();
#endif
                                    if(mgmt_sel>=menu_count&&menu_count>0)mgmt_sel=menu_count-1;
                                    redraw=1;
                                }
                            }
                            else if(ev.code==KEY_S){
                                /* Toggle active */
                                if(mgmt_sel>=0&&mgmt_sel<menu_count){
                                    db_toggle_menu_item(menu_items[mgmt_sel].id);
#ifdef SQLITE_INTEGRATION
                                    db_load_menu();
#else
                                    menu_items[mgmt_sel].active = !menu_items[mgmt_sel].active;
#endif
                                    redraw=1;
                                }
                            }
                            /* Touch: CRUD buttons and row tap-to-select */
                            else if(ev.code==BTN_LEFT||ev.code==BTN_TOUCH){
                                int a=hit_test(mx,my);
                                if(a==750){
                                    /* ADD button tapped */
                                    mgmt_mode=1; mgmt_field=0;
                                    mgmt_name[0]=0; mgmt_price[0]=0; mgmt_cat=0;
                                    kb_show(mgmt_name, 31, 2, 0);
                                    redraw=1;
                                }
                                else if(a==751){
                                    /* EDIT button tapped */
                                    if(mgmt_sel>=0&&mgmt_sel<menu_count){
                                        mgmt_mode=2; mgmt_field=0;
                                        mgmt_edit_id=menu_items[mgmt_sel].id;
                                        strncpy(mgmt_name,menu_items[mgmt_sel].name,31);
                                        snprintf(mgmt_price,sizeof(mgmt_price),"%d",menu_items[mgmt_sel].price);
                                        mgmt_cat=menu_items[mgmt_sel].cat;
                                        kb_show(mgmt_name, 31, 2, 0);
                                    }
                                    redraw=1;
                                }
                                else if(a==752){
                                    /* DELETE button tapped */
                                    if(mgmt_sel>=0&&mgmt_sel<menu_count){
                                        db_delete_menu_item(menu_items[mgmt_sel].id);
#ifdef SQLITE_INTEGRATION
                                        db_load_menu();
#endif
                                        if(mgmt_sel>=menu_count&&menu_count>0)mgmt_sel=menu_count-1;
                                    }
                                    redraw=1;
                                }
                                else if(a==753){
                                    /* TOGGLE button tapped */
                                    if(mgmt_sel>=0&&mgmt_sel<menu_count){
                                        db_toggle_menu_item(menu_items[mgmt_sel].id);
#ifdef SQLITE_INTEGRATION
                                        db_load_menu();
#else
                                        menu_items[mgmt_sel].active = !menu_items[mgmt_sel].active;
#endif
                                    }
                                    redraw=1;
                                }
                                else if(a>=770&&a<770+menu_count){
                                    /* Row tap-to-select */
                                    mgmt_sel=a-770;
                                    redraw=1;
                                }
                                /* Back button is handled above (a==710) */
                            }
                        } else {
                            /* Add/Edit form */
                            if(ev.code==KEY_ESC){kb_hide();mgmt_mode=0;redraw=1;}
                            else if(ev.code==KEY_TAB){mgmt_field=(mgmt_field+1)%3;redraw=1;}
                            else if(ev.code==KEY_LEFT&&mgmt_field==2){mgmt_cat=(mgmt_cat+2)%3;redraw=1;}
                            else if(ev.code==KEY_RIGHT&&mgmt_field==2){mgmt_cat=(mgmt_cat+1)%3;redraw=1;}
                            else if(ev.code==KEY_ENTER||ev.code==KEY_KPENTER){
                                int price=atoi(mgmt_price);
                                if(mgmt_name[0]&&price>0){
                                    if(mgmt_mode==1) db_add_menu_item(mgmt_name,price,mgmt_cat);
                                    else db_update_menu_item(mgmt_edit_id,mgmt_name,price,mgmt_cat);
#ifdef SQLITE_INTEGRATION
                                    db_load_menu();
#endif
                                    mgmt_mode=0;
                                }
                                redraw=1;
                            }
                            else if(ev.code==KEY_BACKSPACE){
                                if(mgmt_field==0) text_input_backspace(mgmt_name);
                                else if(mgmt_field==1) text_input_backspace(mgmt_price);
                                redraw=1;
                            }
                            else {
                                char c=key_to_char(ev.code,shift_held);
                                if(c){
                                    if(mgmt_field==0) text_input_char(mgmt_name,31,c);
                                    else if(mgmt_field==1 && c>='0' && c<='9') text_input_char(mgmt_price,15,c);
                                    redraw=1;
                                }
                            }
                        }
                    }

                    /* ===== USER MANAGEMENT STATE ===== */
                    else if(app_state == STATE_USER_MGMT) {
                        if(umgmt_mode == 0) {
                            if(ev.code==KEY_ESC){kb_hide();app_state=STATE_POS;redraw=1;}
                            else if(ev.code==KEY_UP){if(user_count>0){umgmt_sel--;if(umgmt_sel<0)umgmt_sel=user_count-1;}redraw=1;}
                            else if(ev.code==KEY_DOWN){if(user_count>0){umgmt_sel++;if(umgmt_sel>=user_count)umgmt_sel=0;}redraw=1;}
                            else if(ev.code==KEY_A){
                                umgmt_mode=1; umgmt_field=0;
                                umgmt_uname[0]=0; umgmt_pass[0]=0; umgmt_role=1;
                                kb_show(umgmt_uname, 31, 4, 0);
                                redraw=1;
                            }
                            else if(ev.code==KEY_ENTER||ev.code==KEY_KPENTER){
                                if(umgmt_sel>=0&&umgmt_sel<user_count){
                                    umgmt_mode=2; umgmt_field=0;
                                    umgmt_edit_id=user_list[umgmt_sel].id;
                                    strncpy(umgmt_uname,user_list[umgmt_sel].username,31);
                                    umgmt_pass[0]=0;
                                    umgmt_role=strcmp(user_list[umgmt_sel].role,"admin")==0?0:1;
                                    kb_show(umgmt_uname, 31, 4, 0);
                                    redraw=1;
                                }
                            }
                            else if(ev.code==KEY_D){
                                if(umgmt_sel>=0&&umgmt_sel<user_count){
                                    /* Don't allow deleting yourself */
                                    if(user_list[umgmt_sel].id != current_user_id) {
                                        db_delete_user(user_list[umgmt_sel].id);
                                        db_load_users();
                                        if(umgmt_sel>=user_count&&user_count>0)umgmt_sel=user_count-1;
                                    }
                                    redraw=1;
                                }
                            }
                            /* Touch: CRUD buttons and row tap-to-select */
                            else if(ev.code==BTN_LEFT||ev.code==BTN_TOUCH){
                                int a=hit_test(mx,my);
                                if(a==760){
                                    /* ADD USER button tapped */
                                    umgmt_mode=1; umgmt_field=0;
                                    umgmt_uname[0]=0; umgmt_pass[0]=0; umgmt_role=1;
                                    kb_show(umgmt_uname, 31, 4, 0);
                                    redraw=1;
                                }
                                else if(a==761){
                                    /* EDIT button tapped */
                                    if(umgmt_sel>=0&&umgmt_sel<user_count){
                                        umgmt_mode=2; umgmt_field=0;
                                        umgmt_edit_id=user_list[umgmt_sel].id;
                                        strncpy(umgmt_uname,user_list[umgmt_sel].username,31);
                                        umgmt_pass[0]=0;
                                        umgmt_role=strcmp(user_list[umgmt_sel].role,"admin")==0?0:1;
                                        kb_show(umgmt_uname, 31, 4, 0);
                                    }
                                    redraw=1;
                                }
                                else if(a==762){
                                    /* DELETE button tapped */
                                    if(umgmt_sel>=0&&umgmt_sel<user_count){
                                        if(user_list[umgmt_sel].id != current_user_id) {
                                            db_delete_user(user_list[umgmt_sel].id);
                                            db_load_users();
                                            if(umgmt_sel>=user_count&&user_count>0)umgmt_sel=user_count-1;
                                        }
                                    }
                                    redraw=1;
                                }
                                else if(a>=780&&a<780+user_count){
                                    /* Row tap-to-select */
                                    umgmt_sel=a-780;
                                    redraw=1;
                                }
                            }
                        } else {
                            if(ev.code==KEY_ESC){kb_hide();umgmt_mode=0;redraw=1;}
                            else if(ev.code==KEY_TAB){umgmt_field=(umgmt_field+1)%3;redraw=1;}
                            else if(ev.code==KEY_LEFT&&umgmt_field==2){umgmt_role=1-umgmt_role;redraw=1;}
                            else if(ev.code==KEY_RIGHT&&umgmt_field==2){umgmt_role=1-umgmt_role;redraw=1;}
                            else if(ev.code==KEY_ENTER||ev.code==KEY_KPENTER){
                                const char *role=umgmt_role==0?"admin":"cashier";
                                if(umgmt_uname[0]){
                                    if(umgmt_mode==1 && umgmt_pass[0]) {
                                        db_add_user(umgmt_uname,umgmt_pass,role);
                                    } else if(umgmt_mode==2) {
                                        db_update_user(umgmt_edit_id,umgmt_uname,umgmt_pass,role);
                                    }
                                    db_load_users();
                                    umgmt_mode=0;
                                }
                                redraw=1;
                            }
                            else if(ev.code==KEY_BACKSPACE){
                                if(umgmt_field==0) text_input_backspace(umgmt_uname);
                                else if(umgmt_field==1) text_input_backspace(umgmt_pass);
                                redraw=1;
                            }
                            else {
                                char c=key_to_char(ev.code,shift_held);
                                if(c){
                                    if(umgmt_field==0) text_input_char(umgmt_uname,31,c);
                                    else if(umgmt_field==1) text_input_char(umgmt_pass,31,c);
                                    redraw=1;
                                }
                            }
                        }
                    }

                    /* ===== ORDER HISTORY STATE ===== */
                    else if(app_state == STATE_ORDER_HISTORY) {
                        if(hist_detail==0) {
                            if(ev.code==KEY_ESC){kb_hide();app_state=STATE_POS;redraw=1;}
                            else if(ev.code==KEY_UP){if(hist_count>0){hist_sel--;if(hist_sel<0)hist_sel=hist_count-1;}redraw=1;}
                            else if(ev.code==KEY_DOWN){if(hist_count>0){hist_sel++;if(hist_sel>=hist_count)hist_sel=0;}redraw=1;}
                            else if(ev.code==KEY_ENTER||ev.code==KEY_KPENTER){
                                if(hist_sel>=0&&hist_sel<hist_count){
                                    hist_detail=1;
                                    hist_detail_id=hist_orders[hist_sel].id;
                                    db_load_order_detail(hist_detail_id);
                                    redraw=1;
                                }
                            }
                        } else {
                            if(ev.code==KEY_ESC){kb_hide();hist_detail=0;redraw=1;}
                        }
                    }

                    /* ===== PAID CONFIRM STATE ===== */
                    else if(app_state == STATE_PAID_CONFIRM) {
                        /* Any key or tap goes back to POS */
                        app_state=STATE_POS;
                        redraw=1;
                    }
                }

                /* Mouse movement (all states) */
                if(ev.type==EV_REL){
                    if(ev.code==REL_X)mx+=ev.value;
                    if(ev.code==REL_Y)my+=ev.value;
                    if(mx<0)mx=0;if(my<0)my=0;
                    if(mx>=fb.w)mx=fb.w-1;if(my>=fb.h)my=fb.h-1;
                    cursor_dirty=1;
                }
                else if(ev.type==EV_ABS){
                    if(ev.code==ABS_X||ev.code==ABS_MT_POSITION_X)mx=ev.value*fb.w/32768;
                    if(ev.code==ABS_Y||ev.code==ABS_MT_POSITION_Y)my=ev.value*fb.h/32768;
                    cursor_dirty=1;
                }
            }
        }
    }

done:
    if(tty_fd>=0){ioctl(tty_fd,KDSETMODE,KD_TEXT);close(tty_fd);}
    for(int i=0;i<ninp;i++){ioctl(inp[i],EVIOCGRAB,0);close(inp[i]);}
#ifdef SQLITE_INTEGRATION
    if(db) sqlite3_close(db);
#endif
    free(clean); free(back); munmap(fbmem,fb.size); close(fd);
    return 0;
}
