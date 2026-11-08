/*
 * Orbitron runtime for the LLVM back end.
 *
 * Compiled together with the generated LLVM IR by clang. The JVM back end has
 * an equivalent runtime in OtRt.java; both must produce identical output.
 *
 * Memory model: all heap objects (strings, arrays, structs) are allocated with
 * calloc and released by the OS when the program exits.
 */
#include <errno.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct {
    int64_t len;
    int64_t cap;
    int64_t *data; /* every element occupies one 8-byte slot */
} OtArray;

/* ── Errors ─────────────────────────────────────────────────────────────── */

__attribute__((noreturn)) void ot_panic_at(const char *msg, const char *loc) {
    fflush(stdout);
    if (loc) {
        fprintf(stderr, "ошибка выполнения: %s (%s)\n", msg, loc);
    } else {
        fprintf(stderr, "ошибка выполнения: %s\n", msg);
    }
    exit(101);
}

__attribute__((noreturn)) void ot_panic(const char *msg) {
    fflush(stdout);
    fprintf(stderr, "паника: %s\n", msg);
    exit(101);
}

__attribute__((noreturn)) void ot_exit(int64_t code) {
    fflush(stdout);
    exit((int)code);
}

__attribute__((noreturn)) void ot_assert_fail(const char *msg, const char *loc) {
    fflush(stdout);
    if (msg) {
        fprintf(stderr, "проверка не пройдена: %s (%s)\n", msg, loc);
    } else {
        fprintf(stderr, "проверка не пройдена (%s)\n", loc);
    }
    exit(101);
}

/* ── Memory ─────────────────────────────────────────────────────────────── */

void *ot_alloc(int64_t size) {
    void *p = calloc(1, size > 0 ? (size_t)size : 1);
    if (!p) {
        ot_panic_at("недостаточно памяти", NULL);
    }
    return p;
}

/* ── Arithmetic ─────────────────────────────────────────────────────────── */

int64_t ot_idiv(int64_t a, int64_t b, const char *loc) {
    if (b == 0) ot_panic_at("деление на ноль", loc);
    if (b == -1) return (int64_t)(0 - (uint64_t)a);
    return a / b;
}

int64_t ot_irem(int64_t a, int64_t b, const char *loc) {
    if (b == 0) ot_panic_at("деление на ноль", loc);
    if (b == -1) return 0;
    return a % b;
}

int64_t ot_ipow(int64_t base, int64_t exp, const char *loc) {
    if (exp < 0) ot_panic_at("отрицательная степень целого числа", loc);
    uint64_t result = 1, b = (uint64_t)base;
    while (exp > 0) {
        if (exp & 1) result *= b;
        b *= b;
        exp >>= 1;
    }
    return (int64_t)result;
}

/* Saturating float → int conversion (same rules as Java's (long) cast). */
int64_t ot_f2i(double x) {
    if (x != x) return 0;
    if (x >= 9223372036854775807.0) return INT64_MAX;
    if (x <= -9223372036854775808.0) return INT64_MIN;
    return (int64_t)x;
}

double ot_round(double x) { return round(x); }

double ot_clock(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec / 1e9;
}

/* ── Strings ────────────────────────────────────────────────────────────── */

static char *ot_str_alloc(size_t len) { return (char *)ot_alloc((int64_t)len + 1); }

char *ot_str_concat(const char *a, const char *b) {
    size_t la = strlen(a), lb = strlen(b);
    char *r = ot_str_alloc(la + lb);
    memcpy(r, a, la);
    memcpy(r + la, b, lb);
    return r;
}

int64_t ot_str_eq(const char *a, const char *b) { return strcmp(a, b) == 0; }

int64_t ot_str_cmp(const char *a, const char *b) {
    int c = strcmp(a, b);
    return c < 0 ? -1 : (c > 0 ? 1 : 0);
}

int64_t ot_str_len(const char *s) { return (int64_t)strlen(s); }

int64_t ot_str_byte(const char *s, int64_t i, const char *loc) {
    int64_t n = (int64_t)strlen(s);
    if (i < 0 || i >= n) {
        char buf[128];
        snprintf(buf, sizeof buf, "индекс %lld вне границ строки длины %lld", (long long)i, (long long)n);
        ot_panic_at(buf, loc);
    }
    return (unsigned char)s[i];
}

char *ot_str_slice(const char *s, int64_t a, int64_t b, const char *loc) {
    int64_t n = (int64_t)strlen(s);
    if (a < 0 || b < a || b > n) {
        char buf[160];
        snprintf(buf, sizeof buf, "срез [%lld, %lld) вне границ строки длины %lld", (long long)a, (long long)b, (long long)n);
        ot_panic_at(buf, loc);
    }
    char *r = ot_str_alloc((size_t)(b - a));
    memcpy(r, s + a, (size_t)(b - a));
    return r;
}

int64_t ot_str_contains(const char *s, const char *sub) { return strstr(s, sub) != NULL; }

int64_t ot_str_find(const char *s, const char *sub) {
    const char *p = strstr(s, sub);
    return p ? (int64_t)(p - s) : -1;
}

char *ot_chr(int64_t c, const char *loc) {
    if (c < 0 || c > 0x10FFFF || (c >= 0xD800 && c <= 0xDFFF)) {
        char buf[96];
        snprintf(buf, sizeof buf, "некорректный код символа %lld", (long long)c);
        ot_panic_at(buf, loc);
    }
    char *r = ot_str_alloc(4);
    if (c < 0x80) {
        r[0] = (char)c;
    } else if (c < 0x800) {
        r[0] = (char)(0xC0 | (c >> 6));
        r[1] = (char)(0x80 | (c & 0x3F));
    } else if (c < 0x10000) {
        r[0] = (char)(0xE0 | (c >> 12));
        r[1] = (char)(0x80 | ((c >> 6) & 0x3F));
        r[2] = (char)(0x80 | (c & 0x3F));
    } else {
        r[0] = (char)(0xF0 | (c >> 18));
        r[1] = (char)(0x80 | ((c >> 12) & 0x3F));
        r[2] = (char)(0x80 | ((c >> 6) & 0x3F));
        r[3] = (char)(0x80 | (c & 0x3F));
    }
    return r;
}

char *ot_int_to_str(int64_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%lld", (long long)v);
    char *r = ot_str_alloc((size_t)n);
    memcpy(r, buf, (size_t)n);
    return r;
}

/*
 * Float formatting shared with the JVM runtime:
 *   - shortest decimal that round-trips;
 *   - plain notation for 1e-5 <= |x| < 1e16, always with a fractional part
 *     ("3.0", "0.001");
 *   - otherwise scientific "d.ddde±N" ("1.5e+20", "1e-07" is written "1.0e-7").
 */
static void ot_format_float(double x, char *out, size_t cap) {
    if (x != x) { snprintf(out, cap, "NaN"); return; }
    if (isinf(x)) { snprintf(out, cap, x > 0 ? "inf" : "-inf"); return; }
    if (x == 0) { snprintf(out, cap, signbit(x) ? "-0.0" : "0.0"); return; }

    char sci[64];
    for (int prec = 0; prec < 17; prec++) {
        snprintf(sci, sizeof sci, "%.*e", prec, x);
        if (strtod(sci, NULL) == x) break;
    }
    /* sci = [-]d[.ddd]e[+-]XX → digits and decimal exponent */
    char digits[32];
    int nd = 0, neg = 0;
    const char *p = sci;
    if (*p == '-') { neg = 1; p++; }
    while (*p && *p != 'e') {
        if (*p >= '0' && *p <= '9') digits[nd++] = *p;
        p++;
    }
    digits[nd] = 0;
    int exp10 = atoi(p + 1); /* value = 0.d1d2... * 10^(exp10+1) */
    while (nd > 1 && digits[nd - 1] == '0') digits[--nd] = 0;

    char *o = out;
    size_t left = cap;
#define PUT(c) do { if (left > 1) { *o++ = (c); left--; } } while (0)
    if (neg) PUT('-');
    double ax = fabs(x);
    if (ax >= 1e-5 && ax < 1e16) {
        if (exp10 >= 0) {
            for (int i = 0; i <= exp10; i++) PUT(i < nd ? digits[i] : '0');
            PUT('.');
            if (exp10 + 1 < nd) {
                for (int i = exp10 + 1; i < nd; i++) PUT(digits[i]);
            } else {
                PUT('0');
            }
        } else {
            PUT('0');
            PUT('.');
            for (int i = 0; i < -exp10 - 1; i++) PUT('0');
            for (int i = 0; i < nd; i++) PUT(digits[i]);
        }
        *o = 0;
    } else {
        PUT(digits[0]);
        PUT('.');
        if (nd > 1) {
            for (int i = 1; i < nd; i++) PUT(digits[i]);
        } else {
            PUT('0');
        }
        *o = 0;
        snprintf(o, left, "e%d", exp10);
    }
#undef PUT
}

char *ot_float_to_str(double v) {
    char buf[64];
    ot_format_float(v, buf, sizeof buf);
    size_t n = strlen(buf);
    char *r = ot_str_alloc(n);
    memcpy(r, buf, n);
    return r;
}

/* ── I/O ────────────────────────────────────────────────────────────────── */

/* flags: bit 0 — newline, bit 1 — stderr */
void ot_print_str(const char *s, int64_t flags) {
    FILE *f = (flags & 2) ? stderr : stdout;
    if (flags & 2) fflush(stdout);
    fputs(s, f);
    if (flags & 1) fputc('\n', f);
}

void ot_print_int(int64_t v, int64_t flags) {
    FILE *f = (flags & 2) ? stderr : stdout;
    if (flags & 2) fflush(stdout);
    fprintf(f, "%lld", (long long)v);
    if (flags & 1) fputc('\n', f);
}

void ot_print_float(double v, int64_t flags) {
    char buf[64];
    ot_format_float(v, buf, sizeof buf);
    ot_print_str(buf, flags);
}

char *ot_read_line(void) {
    size_t cap = 64, len = 0;
    char *buf = ot_str_alloc(cap);
    int c;
    while ((c = getchar()) != EOF && c != '\n') {
        if (len + 1 >= cap) {
            char *nb = ot_str_alloc(cap * 2);
            memcpy(nb, buf, len);
            buf = nb;
            cap *= 2;
        }
        buf[len++] = (char)c;
    }
    if (len > 0 && buf[len - 1] == '\r') len--;
    buf[len] = 0;
    return buf;
}

static void ot_parse_error(const char *what, const char *s, const char *loc) {
    char buf[256];
    snprintf(buf, sizeof buf, "не удалось преобразовать \"%.120s\" в %s", s, what);
    ot_panic_at(buf, loc);
}

static const char *ot_skip_ws(const char *s) {
    while (*s == ' ' || *s == '\t' || *s == '\n' || *s == '\r') s++;
    return s;
}

int64_t ot_parse_int(const char *s, const char *loc) {
    const char *p = ot_skip_ws(s);
    char *end;
    if (*p == 0 || *p == '+') ot_parse_error("int", s, loc);
    errno = 0;
    long long v = strtoll(p, &end, 10);
    if (end == p || errno == ERANGE || *ot_skip_ws(end) != 0) ot_parse_error("int", s, loc);
    return (int64_t)v;
}

double ot_parse_float(const char *s, const char *loc) {
    const char *p = ot_skip_ws(s);
    char *end;
    if (*p == 0 || *p == '+') ot_parse_error("float", s, loc);
    /* only plain decimal notation: digits, '.', exponent */
    for (const char *q = p; *q && *q != ' ' && *q != '\t' && *q != '\n' && *q != '\r'; q++) {
        if (!((*q >= '0' && *q <= '9') || *q == '.' || *q == '-' || *q == 'e' || *q == 'E')) {
            ot_parse_error("float", s, loc);
        }
    }
    double v = strtod(p, &end);
    if (end == p || *ot_skip_ws(end) != 0) ot_parse_error("float", s, loc);
    return v;
}

/* ── Arrays ─────────────────────────────────────────────────────────────── */

OtArray *ot_arr_new(int64_t len, const char *loc) {
    if (len < 0) {
        char buf[96];
        snprintf(buf, sizeof buf, "отрицательная длина массива %lld", (long long)len);
        ot_panic_at(buf, loc);
    }
    OtArray *a = (OtArray *)ot_alloc(sizeof(OtArray));
    a->len = len;
    a->cap = len < 4 ? 4 : len;
    a->data = (int64_t *)ot_alloc(a->cap * 8);
    return a;
}

int64_t ot_arr_len(OtArray *a) { return a->len; }

void *ot_arr_slot(OtArray *a, int64_t i, const char *loc) {
    if (i < 0 || i >= a->len) {
        char buf[128];
        snprintf(buf, sizeof buf, "индекс %lld вне границ массива длины %lld", (long long)i, (long long)a->len);
        ot_panic_at(buf, loc);
    }
    return &a->data[i];
}

/* Unchecked slot access for compiler-generated loops and literals. */
void *ot_arr_data(OtArray *a) { return a->data; }

void *ot_arr_push_slot(OtArray *a) {
    if (a->len == a->cap) {
        int64_t ncap = a->cap * 2;
        int64_t *nd = (int64_t *)ot_alloc(ncap * 8);
        memcpy(nd, a->data, (size_t)a->len * 8);
        free(a->data);
        a->data = nd;
        a->cap = ncap;
    }
    return &a->data[a->len++];
}

void *ot_arr_pop_slot(OtArray *a, const char *loc) {
    if (a->len == 0) ot_panic_at("pop() из пустого массива", loc);
    return &a->data[--a->len];
}
