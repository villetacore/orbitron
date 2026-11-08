# Orbitron

**Orbitron** — статически типизированный компилируемый язык программирования с лаконичным синтаксисом и двумя бэкендами:

- **LLVM** — нативный исполняемый файл (через LLVM IR и clang);
- **JVM** — переносимый `.jar` (через генерацию Java и javac).

Обе сборки одной программы печатают одно и то же, байт в байт: у них общая семантика, одинаковое форматирование чисел и одинаковые сообщения об ошибках выполнения.

```orbitron
import "std/arrays";

struct Planet {
    name: str,
    radius_km: float,
}

impl Planet {
    fn area(self) -> float { 4.0 * 3.141592653589793 * self.radius_km * self.radius_km }
}

fn main() {
    let planets = [
        Planet { name: "Меркурий", radius_km: 2439.7 },
        Planet { name: "Земля", radius_km: 6371.0 },
    ];
    for p in planets {
        let size = if p.radius_km > 5000.0 { "большая" } else { "маленькая" };
        println("{p.name}: {size}, площадь {p.area() as int} км²");
    }

    let squares = map(range(1, 6), fn(x: int) -> int { x * x });
    println(to_str(squares)); // [1, 4, 9, 16, 25]
}
```

## Возможности

- статическая типизация с выводом типов локальных переменных: `int`, `float`, `bool`, `str`, массивы `[T]`, структуры, перечисления, функциональные типы `fn(int) -> int`;
- `let` для неизменяемых привязок, `var` для изменяемых переменных, `const` для констант времени компиляции;
- всё, что может вернуть значение, является выражением: `if`, `match`, блоки `{ ... }`, тело функции;
- `match` по числам, диапазонам, строкам, `bool` и вариантам перечислений, с проверкой полноты;
- структуры с методами (`impl`), функции типа (`Point::new`), методы у перечислений;
- функции как значения, анонимные функции `fn(x: int) -> int { x * 2 }`, конвейер `x |> f(a)`;
- интерполяция любых выражений в строках: `"сумма = {a + b}"`;
- `defer` с областью действия блока;
- массивы с проверкой границ, ошибки выполнения с указанием файла и строки;
- понятные сообщения об ошибках компиляции на русском языке;
- форматтер `orbitron fmt`, проекты `orbitron.toml`, стандартная библиотека;
- вызов функций C через `extern fn` (в LLVM-бэкенде).

## Установка

Сам компилятор написан на Rust и не требует LLVM при сборке:

```bash
git clone https://github.com/villetacore/orbitron
cd orbitron
cargo build --release
```

Для работы с бэкендами нужны:

| Бэкенд | Что нужно | Установка |
|---|---|---|
| `llvm` (по умолчанию) | `clang` | macOS: `xcode-select --install` · Ubuntu: `sudo apt install clang` |
| `jvm` | JDK 17+ (`javac`, `jar`, `java`) | Ubuntu: `sudo apt install openjdk-21-jdk` |

Компилятор ищет стандартную библиотеку в `$ORBITRON_HOME/stdlib`, рядом с исполняемым файлом и в каталоге исходников. Для установки в систему:

```bash
sudo install -Dm755 target/release/orbitron /usr/local/bin/orbitron
sudo mkdir -p /usr/local/share/orbitron && sudo cp -r stdlib /usr/local/share/orbitron/
```

## Быстрый старт

```bash
orbitron run examples/01_hello.ot                  # собрать и запустить (LLVM)
orbitron run examples/01_hello.ot --backend jvm    # то же через JVM
orbitron build examples/09_life.ot -o life         # собрать ./life
orbitron check programm.ot                         # только проверить типы
orbitron fmt                                       # отформатировать все .ot в проекте

orbitron new hello && cd hello && orbitron run     # новый проект
```

## Документация

- [Язык Orbitron](docs/language.md) — полное описание синтаксиса и семантики
- [Стандартная библиотека](docs/stdlib.md)
- [Компилятор, проекты и бэкенды](docs/tooling.md)
- [Устройство компилятора](docs/internals.md)
- [Переход со старого синтаксиса](docs/migration.md)
- [Примеры](examples/)

## Разработка

```bash
cargo test     # модульные тесты и сквозные тесты на обоих бэкендах
```

Сквозные тесты собирают каждую программу из `tests/programs` через LLVM и JVM и сравнивают вывод с эталоном. JVM-часть пропускается, если не установлен `javac`. Подробности в [CONTRIBUTING.md](CONTRIBUTING.md).

## Лицензия

[MIT](LICENSE) © 2026 Виллета Корь
