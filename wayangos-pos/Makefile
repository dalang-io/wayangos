# WayangPOS — static framebuffer POS build
# Usage: make            (auto-detects sqlite3.c for persistence)
#        make clean

CC      = gcc
CFLAGS  ?= -O2
BUILD   ?= $(HOME)/wayangos-build
TARGET   = wayang-pos
SRC      = fbpos-v3.c

# Optional SQLite amalgamation: repo root or the build directory.
SQLITE   := $(firstword $(wildcard ../sqlite3.c $(BUILD)/sqlite3.c))

ifneq ($(SQLITE),)
SQLITE_FLAGS = -DSQLITE_INTEGRATION
SQLITE_INC   = -I$(dir $(SQLITE))
SQLITE_LIBS  = -lpthread
endif

.PHONY: all clean

all: $(TARGET)

$(TARGET): $(SRC)
	$(CC) -static $(CFLAGS) $(SQLITE_INC) -o $@ $(SRC) $(SQLITE) -lm $(SQLITE_LIBS) $(SQLITE_FLAGS)

clean:
	rm -f $(TARGET)
