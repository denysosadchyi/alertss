# Сторінка SirenBar у Microsoft Store (українська, uk-UA)

## Опис (Description)

SirenBar — невеликий віджет повітряних тривог, який живе прямо на панелі задач Windows 11, там, де зазвичай погода. Один погляд униз — і ви знаєте, чи є тривога у вашій області.

• Жовта тривога — атака дронів, червона — ракетна небезпека, відбій — можна видихнути.
• Клік по віджету відкриває карту України: усі області кольором за рівнем тривоги, райони й громади під загрозою, тривалість тривоги та журнал за день.
• Свою область обираєте кліком по карті. Київ і Севастополь — окремо від області.
• Сповіщення Windows про початок, відбій і зміну рівня тривоги (зі звуком, вимикається).
• Світла й темна тема, коректне масштабування екрана, автозапуск разом з Windows.

Дані — з відкритих джерел ubilling.net.ua та alerts.in.ua; можна вказати власний токен офіційного API alerts.in.ua. Програма не збирає жодних персональних даних, не визначає геолокацію і не містить реклами.

SirenBar — незалежний проєкт з відкритим кодом і не є офіційним застосунком державних служб чи alerts.in.ua. Не покладайтеся лише на нього: зважайте на сирени та офіційні канали оповіщення.

## Функції продукту (Product features)

- Статус повітряної тривоги вашої області прямо на панелі задач
- Жовта (дрони) і червона (ракети) тривога, часткова тривога по районах
- Карта України з усіма тривогами в один клік
- Вибір області кліком по карті
- Сповіщення Windows про тривогу, відбій і зміну рівня
- Журнал тривог за день
- Світла й темна тема Windows 11
- Без реклами, без збору даних, відкритий код

## Що нового (What's new in this version)

Перший реліз у Microsoft Store.

## Пошукові терміни (Search terms, до 7)

тривога; повітряна тривога; air raid alert; Україна; сирена; дрони; ракети

## Інше

- Категорія: Utilities & tools (Утиліти та інструменти)
- Ціна: безкоштовно, усі ринки
- Політика конфіденційності: https://github.com/denysosadchyi/sirenbar/blob/main/PRIVACY.md
- Сайт: https://github.com/denysosadchyi/sirenbar
- Підтримка: https://github.com/denysosadchyi/sirenbar/issues
- Copyright: © 2026 Denys Osadchyi

## Пояснення для restricted capability runFullTrust (Submission options)

SirenBar is a classic Win32 desktop app (Rust, Win32 API) packaged as MSIX. runFullTrust is required because it is a full-trust desktop executable: it shows a small status widget docked on the Windows taskbar and a flyout window with a map, and uses a StartupTask to start with Windows. It does not access user files or personal data; it only downloads public air-raid alert data over HTTPS.
