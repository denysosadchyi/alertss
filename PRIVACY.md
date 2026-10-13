# Політика конфіденційності SirenBar

*Оновлено: 13 жовтня 2026*

SirenBar — віджет повітряних тривог для панелі задач Windows. Він **не збирає, не зберігає й не передає жодних персональних даних** користувача.

## Що програма робить у мережі

Щоб показати актуальні тривоги, SirenBar періодично завантажує загальнодоступні дані про тривоги в Україні:

- [ubilling.net.ua/aerialalerts](https://ubilling.net.ua/aerialalerts/) — статус по областях;
- [alerts.in.ua](https://alerts.in.ua/) — відкритий звіт про тривоги та, якщо ви самі вкажете власний токен, офіційний API alerts.in.ua.

Ці запити однакові для всіх користувачів і не містять жодної інформації про вас, ваш пристрій чи місцезнаходження. Програма не визначає геолокацію — область ви обираєте вручну.

## Що зберігається на вашому комп'ютері

Лише налаштування: обрана область, звук, анімація, автозапуск і (за бажанням) ваш токен alerts.in.ua. Вони зберігаються локально у файлі `config.json` і нікуди не надсилаються, окрім самого токена, який передається лише в API alerts.in.ua для авторизації запиту.

## Аналітика, реклама, сторонні сервіси

Немає. SirenBar не містить аналітики, телеметрії, реклами чи трекерів.

## Контакти

Питання та зауваження: [github.com/denysosadchyi/sirenbar/issues](https://github.com/denysosadchyi/sirenbar/issues).

---

# SirenBar Privacy Policy (English)

SirenBar does not collect, store or transmit any personal data. It only downloads public air-raid alert data for Ukraine (ubilling.net.ua, alerts.in.ua). Settings (selected region, sound, autostart and an optional alerts.in.ua API token) are stored locally on your computer; the token is sent only to the alerts.in.ua API. No analytics, telemetry, ads or trackers. Contact: https://github.com/denysosadchyi/sirenbar/issues
