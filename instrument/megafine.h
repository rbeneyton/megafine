/*
 * megafine region-timing instrumentation (C / C++).
 *
 * Bracket the region you want timed with megafine_start() / megafine_stop().
 * The calls are no-ops unless the program is run under `megafine --region`
 * (which sets MEGAFINE_FD in the environment), so the instrumented binary still
 * builds and runs normally on its own.
 *
 *     #include "megafine.h"
 *     ...
 *     megafine_start();
 *     // ... code to measure ...
 *     megafine_stop();
 */
#ifndef MEGAFINE_H
#define MEGAFINE_H

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Emit one 9-byte event [tag:u8][ns:u64 native-endian] to MEGAFINE_FD.
 * 9 < PIPE_BUF, so the write is atomic and safe from multiple threads. */
static inline void megafine_emit_(uint8_t tag) {
    const char *s = getenv("MEGAFINE_FD");
    if (!s) {
        return; /* standalone run: no-op, no I/O */
    }
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t ns = (uint64_t)ts.tv_sec * 1000000000ull + (uint64_t)ts.tv_nsec;
    unsigned char buf[9];
    buf[0] = tag;
    memcpy(buf + 1, &ns, 8);
    ssize_t n = write(atoi(s), buf, sizeof buf);
    (void)n;
}

static inline void megafine_start(void) { megafine_emit_(0); }
static inline void megafine_stop(void) { megafine_emit_(1); }

#ifdef __cplusplus
}
#endif

#endif /* MEGAFINE_H */
