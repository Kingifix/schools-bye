# schools.bye
Android-клиент schools.by. Tauri 2: Rust (`src-tauri/src/lib.rs` команды/HTTP, `parse.rs` HTML-парсинг) + один TS-файл `src/main.ts` (UI), `src/styles.css`, `index.html`. Kotlin: `src-tauri/gen/android/.../FileViewerPlugin.kt`. CI: `.github/workflows/release.yml`.
Детали, механизмы, релиз: `docs/CONTEXT.md` (читай по необходимости; при смене архитектуры/CI обнови его).

## Проверка
```bash
source /d/dev-tools/tauri-env.sh   # bun/cargo в PATH
bun x tsc --noEmit -p .            # типы фронта
bun tauri android build --apk --target aarch64   # Rust+Kotlin (cargo check на хосте не работает)
```

## Правила
- Версию поднимать в `package.json` и `src-tauri/Cargo.toml`; push в `main` = релиз.
- Сабжекты коммитов — по-русски, понятно школьнику (идут в заметки релиза).
- Ключ подписи не в git; `src-tauri/gen/android` коммитить.
