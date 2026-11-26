# schools.bye — подробный контекст

Неофициальный Android-клиент дневника schools.by. Tauri 2: Rust-бэкенд ходит на сайт/API, фронт — один TS-файл без фреймворка. Только Android (aarch64); десктоп не собирается (нет windres).

## Карта файлов
| файл | что внутри |
| --- | --- |
| `src-tauri/src/lib.rs` | все `#[tauri::command]`, HTTP, сессия. Секции `// --- NAME ---`: STATE, REST API, AUTH, SCHEDULE, LESSON DETAILS, NEW MARKS, FILE DOWNLOAD, SELF-UPDATE, FINAL, ENTRY POINT (регистрация команд в `generate_handler!`) |
| `src-tauri/src/parse.rs` | синхронный HTML-парсинг (scraper) страниц schools.by → serde-структуры (`Week`, `Quarter`, `OfficialMarks`, `NewMark`…) |
| `src/main.ts` | весь UI. Секции `// ---------- name ----------`: types, helpers (`$`, `store`=localStorage, `escapeHtml`, `linkify`, `toast`, `handleError`), schedule, lesson details, new marks, bottom sheet (`openSheet`/`closeSheet`), final, goal, self-update, auth |
| `src/styles.css` | тёмная тема, токены в `:root` (`--card`, `--card-2`, `--accent`, `--header-h`…) |
| `index.html` | статичный каркас (хедер, `#update-banner`, `#schedule`, модалка логина) |
| `src-tauri/gen/android/app/src/main/java/com/koder/schoolsbye/FileViewerPlugin.kt` | Kotlin-плагин: `open` (chooser), `share` (долгое нажатие на файл), `install` (APK-установщик с проверкой подписи). Вызывается из Rust через `run_mobile_plugin` |
| `.github/workflows/release.yml` | CI-релиз |
| `.github/scripts/report.py` | блок «Проверка» в релизе: SHA-256, трекеры (εxodus), VirusTotal |

## Ключевые механизмы
- **HTTP-клиент** в `AppState.client` с `redirect: none` — редиректы обходим вручную (циклы с `LOCATION`). Ошибки сети → префикс `NETWORK`, истёкшая сессия → `SESSION_EXPIRED`; фронт разбирает в `handleError`.
- **Две авторизации**: cookie сайта (`Session.cookie`, `pupil_id`, `domain`) + токен `api.schools.by` (HMAC в `api_digest`, как у офиц. приложения). Токена может не быть (`NO_API_TOKEN`) — фронт деградирует.
- **Кэш на фронте**: `store` (localStorage), `weekCache`; `logout` делает `store.clear()`.
- **Самообновление**: `check_update` берёт `releases/latest` с GitHub, ищет `.apk` и его `digest` (sha256; без него обновление не предлагается). `install_update` качает только с github.com/*.githubusercontent.com, сверяет SHA-256, затем Kotlin `install` сверяет packageName и сертификат подписи (ошибки → `UPDATE_VERIFY`). Фронт: проверка при старте и при возврате в приложение (не чаще раза в 30 мин), результат кэшируется в `update-info`; заметки релиза рендерит мини-Markdown `markdown()`.

## Релиз
1. Поднять версию в `package.json` **и** `src-tauri/Cargo.toml` (tauri.conf.json читает package.json).
2. Push в `main` → Actions соберёт подписанный APK, создаст `vX.Y.Z`.
3. Заметки релиза автоматические: «Что нового» = сабжекты коммитов с прошлого тега (`ci:`/`chore:`/`deps:`/`build:`/`docs:` скрыты, префикс `feat:`/`fix:` срезается) → ссылка на diff → блок проверок. **Сабжекты пользовательских коммитов — по-русски, понятно школьнику; подробности в теле.**
4. Кэши CI: rust-cache, gradle (setup-java), bun, NDK.

## Ловушки
- Ключ подписи (`*.jks`, `keystore.properties`) не в git; смена ключа ломает самообновление.
- `src-tauri/gen/android` в git — правки Kotlin коммитить.
- Python-генерация TS через heredoc съедает `\n`/`\r` — проверяй tsc.
- `cargo check` на хосте падает (windres / нет clang для ring) — проверяй через android build.
