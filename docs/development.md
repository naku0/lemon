# Разработка

Проверки:

```bash
cargo fmt --all --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked
python3 scripts/smoke.py
python3 scripts/report_smoke.py
```

`smoke.py` проверяет TUI в PTY, запись, метки, паузу, Ctrl+C и replay. Rust-тесты покрывают декодеры, DSP, storage, DMOV, report и Settings. Тесты не используют реальные биосигналы.

Перед изменениями проверьте:

```bash
git status --short
```

Не добавляйте в Git записи из `Downloads`, локальные отчёты, временные дампы и пользовательские настройки. Изменения лучше разделять на небольшие коммиты: импорт, Serial, analysis/report, TUI.

Для просмотра CLI:

```bash
lemon --help
lemon live --help
lemon probe --help
lemon analyze --help
lemon report --help
```
