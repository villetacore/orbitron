// Orbitron runtime for the JVM back end.
//
// Mirrors runtime/orbitron_rt.c: every operation must behave and print exactly
// like the native runtime (strings are measured and sliced in UTF-8 bytes,
// floats use the same formatting, runtime errors use the same messages).

import java.io.BufferedOutputStream;
import java.io.BufferedReader;
import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.InputStreamReader;
import java.io.PrintStream;
import java.math.BigDecimal;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;

final class OtRt {
    private OtRt() {}

    static final PrintStream OUT = new PrintStream(
            new BufferedOutputStream(new FileOutputStream(FileDescriptor.out), 1 << 16), false, StandardCharsets.UTF_8);
    static final PrintStream ERR = new PrintStream(
            new FileOutputStream(FileDescriptor.err), true, StandardCharsets.UTF_8);
    private static BufferedReader in;
    private static final long START = System.nanoTime();

    /** Thrown to unwind after System.exit has been requested (never observed). */
    static final class Halt extends RuntimeException {
        Halt() { super(null, null, false, false); }
    }

    // ── Program entry ────────────────────────────────────────────────────────

    static void run(Runnable body) {
        Thread t = new Thread(null, () -> {
            try {
                body.run();
                halt(0);
            } catch (Halt h) {
                // exit already requested
            } catch (StackOverflowError e) {
                OUT.flush();
                ERR.println("ошибка выполнения: переполнение стека (слишком глубокая рекурсия)");
                System.exit(101);
            }
        }, "orbitron-main", 1L << 30);
        t.start();
        try {
            t.join();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    private static Halt halt(long code) {
        OUT.flush();
        System.exit((int) code);
        return new Halt();
    }

    static RuntimeException exit(long code) { return halt(code); }

    static RuntimeException panic(String msg) {
        OUT.flush();
        ERR.println("паника: " + msg);
        return halt(101);
    }

    static RuntimeException panicAt(String msg, String loc) {
        OUT.flush();
        ERR.println(loc == null ? "ошибка выполнения: " + msg : "ошибка выполнения: " + msg + " (" + loc + ")");
        return halt(101);
    }

    static RuntimeException assertFail(String msg, String loc) {
        OUT.flush();
        ERR.println(msg == null ? "проверка не пройдена (" + loc + ")" : "проверка не пройдена: " + msg + " (" + loc + ")");
        return halt(101);
    }

    static RuntimeException unreachable() { return new IllegalStateException("unreachable"); }

    static boolean b(boolean v) { return v; }

    static void ignore(Object o) {}

    // ── Arithmetic ───────────────────────────────────────────────────────────

    static long div(long a, long b, String loc) {
        if (b == 0) throw panicAt("деление на ноль", loc);
        return a / b; // Long.MIN_VALUE / -1 wraps, as in the native runtime
    }

    static long rem(long a, long b, String loc) {
        if (b == 0) throw panicAt("деление на ноль", loc);
        return a % b;
    }

    static long pow(long base, long exp, String loc) {
        if (exp < 0) throw panicAt("отрицательная степень целого числа", loc);
        long result = 1, b = base;
        while (exp > 0) {
            if ((exp & 1) != 0) result *= b;
            b *= b;
            exp >>= 1;
        }
        return result;
    }

    static long min(long a, long b) { return a < b ? a : b; }
    static long max(long a, long b) { return a > b ? a : b; }
    static double min(double a, double b) { return a < b ? a : b; }
    static double max(double a, double b) { return a > b ? a : b; }

    /** C round(): halfway cases away from zero. */
    static double round(double x) {
        if (Double.isNaN(x) || Double.isInfinite(x) || Math.abs(x) >= 4503599627370496.0) return x;
        double f = Math.floor(x);
        double diff = x - f;
        double r;
        if (diff > 0.5) r = f + 1;
        else if (diff < 0.5) r = f;
        else r = x > 0 ? f + 1 : f;
        return Math.copySign(r, x);
    }

    static double clock() { return (System.nanoTime() - START) / 1e9; }

    // ── Strings (UTF-8 semantics) ────────────────────────────────────────────

    static long len(String s) {
        long n = 0;
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            if (c < 0x80) n += 1;
            else if (c < 0x800) n += 2;
            else if (Character.isHighSurrogate(c) && i + 1 < s.length() && Character.isLowSurrogate(s.charAt(i + 1))) {
                n += 4;
                i++;
            } else n += 3;
        }
        return n;
    }

    static long byteAt(String s, long i, String loc) {
        byte[] b = s.getBytes(StandardCharsets.UTF_8);
        if (i < 0 || i >= b.length) {
            throw panicAt("индекс " + i + " вне границ строки длины " + b.length, loc);
        }
        return b[(int) i] & 0xFF;
    }

    static String slice(String s, long a, long e, String loc) {
        byte[] b = s.getBytes(StandardCharsets.UTF_8);
        if (a < 0 || e < a || e > b.length) {
            throw panicAt("срез [" + a + ", " + e + ") вне границ строки длины " + b.length, loc);
        }
        return new String(b, (int) a, (int) (e - a), StandardCharsets.UTF_8);
    }

    static long find(String s, String sub) {
        int i = s.indexOf(sub);
        return i < 0 ? -1 : len(s.substring(0, i));
    }

    static long cmp(String a, String b) {
        byte[] x = a.getBytes(StandardCharsets.UTF_8);
        byte[] y = b.getBytes(StandardCharsets.UTF_8);
        int n = Math.min(x.length, y.length);
        for (int i = 0; i < n; i++) {
            int d = (x[i] & 0xFF) - (y[i] & 0xFF);
            if (d != 0) return d < 0 ? -1 : 1;
        }
        return Integer.compare(x.length, y.length);
    }

    static String chr(long c, String loc) {
        if (c < 0 || c > 0x10FFFF || (c >= 0xD800 && c <= 0xDFFF)) {
            throw panicAt("некорректный код символа " + c, loc);
        }
        return new String(Character.toChars((int) c));
    }

    /** Same algorithm as ot_format_float in the C runtime. */
    static String fstr(double x) {
        if (Double.isNaN(x)) return "NaN";
        if (Double.isInfinite(x)) return x > 0 ? "inf" : "-inf";
        if (x == 0) return (1 / x < 0) ? "-0.0" : "0.0";
        BigDecimal d = new BigDecimal(Double.toString(Math.abs(x))).stripTrailingZeros();
        String digits = d.unscaledValue().toString();
        int exp10 = digits.length() - 1 - d.scale(); // value = d1.d2d3... * 10^exp10
        StringBuilder sb = new StringBuilder();
        if (x < 0) sb.append('-');
        double ax = Math.abs(x);
        int nd = digits.length();
        if (ax >= 1e-5 && ax < 1e16) {
            if (exp10 >= 0) {
                for (int i = 0; i <= exp10; i++) sb.append(i < nd ? digits.charAt(i) : '0');
                sb.append('.');
                if (exp10 + 1 < nd) sb.append(digits, exp10 + 1, nd);
                else sb.append('0');
            } else {
                sb.append("0.");
                for (int i = 0; i < -exp10 - 1; i++) sb.append('0');
                sb.append(digits);
            }
        } else {
            sb.append(digits.charAt(0)).append('.');
            if (nd > 1) sb.append(digits, 1, nd);
            else sb.append('0');
            sb.append('e').append(exp10);
        }
        return sb.toString();
    }

    // ── I/O ──────────────────────────────────────────────────────────────────

    static void print(String s, long flags) {
        if ((flags & 2) != 0) {
            OUT.flush();
            ERR.print(s);
            if ((flags & 1) != 0) ERR.print('\n');
            ERR.flush();
        } else {
            OUT.print(s);
            if ((flags & 1) != 0) OUT.print('\n');
        }
    }

    static void print(long v, long flags) { print(Long.toString(v), flags); }

    static void print(double v, long flags) { print(fstr(v), flags); }

    static String readLine() {
        try {
            if (in == null) in = new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));
            OUT.flush();
            String s = in.readLine();
            if (s == null) return "";
            if (s.endsWith("\r")) s = s.substring(0, s.length() - 1);
            return s;
        } catch (java.io.IOException e) {
            return "";
        }
    }

    private static String trimWs(String s) {
        int a = 0, e = s.length();
        while (a < e && " \t\n\r".indexOf(s.charAt(a)) >= 0) a++;
        while (e > a && " \t\n\r".indexOf(s.charAt(e - 1)) >= 0) e--;
        return s.substring(a, e);
    }

    private static RuntimeException parseError(String what, String s, String loc) {
        String shown = s.length() > 120 ? s.substring(0, 120) : s;
        return panicAt("не удалось преобразовать \"" + shown + "\" в " + what, loc);
    }

    static long parseInt(String s, String loc) {
        String t = trimWs(s);
        if (!t.matches("-?[0-9]+")) throw parseError("int", s, loc);
        try {
            return Long.parseLong(t);
        } catch (NumberFormatException e) {
            throw parseError("int", s, loc);
        }
    }

    static double parseFloat(String s, String loc) {
        String t = trimWs(s);
        if (t.isEmpty() || !t.matches("[0-9.eE-]+")) throw parseError("float", s, loc);
        try {
            return Double.parseDouble(t);
        } catch (NumberFormatException e) {
            throw parseError("float", s, loc);
        }
    }

    // ── Arrays ───────────────────────────────────────────────────────────────

    @SafeVarargs
    static <T> ArrayList<T> list(T... xs) {
        ArrayList<T> a = new ArrayList<>(Math.max(4, xs.length));
        for (T x : xs) a.add(x);
        return a;
    }

    static <T> ArrayList<T> repeat(T v, long n, String loc) {
        if (n < 0) throw panicAt("отрицательная длина массива " + n, loc);
        if (n > Integer.MAX_VALUE - 8) throw panicAt("недостаточно памяти", null);
        ArrayList<T> a = new ArrayList<>((int) Math.max(4, n));
        for (long i = 0; i < n; i++) a.add(v);
        return a;
    }

    static <T> T get(ArrayList<T> a, long i, String loc) {
        if (i < 0 || i >= a.size()) {
            throw panicAt("индекс " + i + " вне границ массива длины " + a.size(), loc);
        }
        return a.get((int) i);
    }

    static <T> void set(ArrayList<T> a, long i, T v, String loc) {
        if (i < 0 || i >= a.size()) {
            throw panicAt("индекс " + i + " вне границ массива длины " + a.size(), loc);
        }
        a.set((int) i, v);
    }

    static <T> T pop(ArrayList<T> a, String loc) {
        if (a.isEmpty()) throw panicAt("pop() из пустого массива", loc);
        return a.remove(a.size() - 1);
    }
}
