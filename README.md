# SirenBar — повітряні тривоги на панелі задач Windows 11

Маленький віджет, який живе прямо на панелі задач (там, де зазвичай погода) і показує статус повітряної тривоги для вибраної області. Клік відкриває панель з картою України.

<p align="center">
  <img src="https://raw.githubusercontent.com/denysosadchyi/sirenbar/main/docs/clear.png" alt="Немає тривоги" width="240">
  <img src="https://raw.githubusercontent.com/denysosadchyi/sirenbar/main/docs/yellow.png" alt="Жовта тривога — дрони" width="240">
  <img src="https://raw.githubusercontent.com/denysosadchyi/sirenbar/main/docs/red.png" alt="Червона тривога — ракети" width="240">
</p>
<p align="center"><sub>Немає тривоги · Жовта — дрони (частина області) · Червона — ракети</sub></p>

## Завантажити

**[sirenbar-setup.exe](https://github.com/denysosadchyi/sirenbar/releases/latest/download/sirenbar-setup.exe)** — інсталятор (Windows 11 x64). Права адміністратора не потрібні: програма ставиться у `%LOCALAPPDATA%\SirenBar\`, за бажанням додає автозапуск і ярлик у «Пуск», видаляється через *Параметри → Програми*.

Без інсталятора — окремий **[sirenbar.exe](https://github.com/denysosadchyi/sirenbar/releases/latest/download/sirenbar.exe)**: покладіть у будь-яку папку й запустіть; автозапуск вмикається в меню правого кліку.

Файли не підписані, тож Windows SmartScreen може попередити: «Докладніше» → «Однаково запустити». Усі версії — на сторінці [Releases](https://github.com/denysosadchyi/sirenbar/releases).

## Можливості

- **Віджет на таскбарі** — іконка + статус і тривалість («Дрони · 0:21», «Ракети · 1:05», «Немає тривоги»), назва області. Під час тривоги іконка м'яко пульсує.
- **Рівні тривоги** — жовта (дрони, власна іконка Шахеда) і червона (ракети), а також часткова тривога, коли тривога лише в окремих районах чи громадах області.
- **Панель з картою** — усі області кольором за рівнем тривоги, підказка при наведенні, вибір своєї області кліком по карті (м. Київ і Севастополь — окремо від області).
- **Журнал за сьогодні** — коли почалась тривога, коли відбій, коли змінився рівень.
- **Сповіщення Windows** про тривогу, відбій і зміну рівня (зі звуком, вимикається).
- **Світла й темна тема**, акрилова панель із заокругленими кутами, коректне масштабування (100–200 %).
- **Автозапуск** разом з Windows; після перезапуску Провідника віджет сам повертається на місце.

## Джерела даних

- [ubilling.net.ua/aerialalerts](https://ubilling.net.ua/aerialalerts/) — статус по областях, без ключа, кожні 15 с.
- [alerts.in.ua](https://alerts.in.ua/) — відкритий звіт SITREP (рівні тривог, райони й громади), кожні 60 с.
- Офіційний API alerts.in.ua — якщо вказати власний токен (ПКМ → «Токен alerts.in.ua…»).

Карта областей згенерована з Natural Earth (public domain) скриптом `tools/gen_map.py`.

## Збірка з коду

Потрібен Windows 11 і Rust (GNU або MSVC toolchain).

```sh
cargo build --release
```

Готовий файл — `target/release/sirenbar.exe`. Скопіюйте його куди зручно (наприклад, `%LOCALAPPDATA%\SirenBar\`) і запустіть. Вбудований віджет погоди краще вимкнути: *Параметри → Персоналізація → Панель завдань → Віджети*.

## Керування

- **Лівий клік** по віджету — панель з картою; Esc або клік поза нею — закрити.
- **Клік по області на карті** — стежити за нею.
- **Правий клік** — меню: область, звук, анімація, автозапуск, токен, карта тривог, вихід.

Налаштування зберігаються в `%APPDATA%\SirenBar\config.json`.

## Розробка

```sh
cargo test                       # тести парсерів, іконок, геометрії карти
cargo run --release -- --fake-alert   # демо: стани змінюються кожні 20 с
```

Інсталятор збирається [Inno Setup](https://jrsoftware.org/isinfo.php) 6 після `cargo build --release`:

```sh
ISCC installer/sirenbar.iss       # -> target/installer/sirenbar-setup.exe
```

Іконки — [Fluent UI System Icons](https://github.com/microsoft/fluentui-system-icons) (MIT), іконки дрона й ракети намальовані власноруч (див. `assets/icons/README.txt`).
