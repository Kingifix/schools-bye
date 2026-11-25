<img src="assets/icon.svg" width="64" height="64" alt="">

# schools.bye

Быстрый неофициальный клиент дневника [schools.by](https://schools.by) для Android (Tauri 2 + Rust + TypeScript).

- расписание и д/з по неделям, темы уроков и заметки учителя, вложения;
- новые отметки;
- итоги: все отметки текущей четверти, четвертные/полугодовые и годовые (свои средние, пока учитель не выставил), цель — какие отметки нужно получить, чтобы выйти на нужную.

## Релиз

Версия берётся из `package.json` (её же читает `tauri.conf.json`; в `src-tauri/Cargo.toml` держим такую же).
Поднять версию → push в `main` → GitHub Actions соберёт подписанный APK и выложит релиз `vX.Y.Z`.

Секреты репозитория (Settings → Secrets and variables → Actions):

| секрет | что это |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | `base64 -w0 my-release-key.jks` |
| `ANDROID_KEY_ALIAS` | alias ключа |
| `ANDROID_KEY_PASSWORD` | пароль хранилища и ключа |

## Локальная сборка (arm64)

```bash
source /d/dev-tools/tauri-env.sh
bun install
bun tauri android build --apk --target aarch64
# -> src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk
```

Подпись: `src-tauri/gen/android/keystore.properties` (не в git) → `my-release-key.jks`.
Иконка: `src-tauri/icon-src/icon.svg` → `bun tauri icon src-tauri/icon-src/icon.svg`.
