CC = x86_64-w64-mingw32-gcc
CFLAGS = -O2 -Wall -Wextra -std=c11 -D_WIN32_WINNT=0x0601
LDFLAGS = -static -s

all: prp_bridge.exe prp_test_client.exe tgi_test_client.exe

# GUI subsystem: Proton's steam.exe gives console programs a new console window.
prp_bridge.exe: prp_bridge.c Makefile
	$(CC) $(CFLAGS) -mwindows -o $@ $< $(LDFLAGS) -lws2_32 -lshell32

prp_test_client.exe: prp_test_client.c
	$(CC) $(CFLAGS) -o $@ $< $(LDFLAGS)

tgi_test_client.exe: tgi_test_client.c
	$(CC) $(CFLAGS) -o $@ $< $(LDFLAGS) -luser32

clean:
	rm -f prp_bridge.exe prp_test_client.exe tgi_test_client.exe

.PHONY: all clean
