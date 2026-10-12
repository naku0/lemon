# Разработка

## Первый запуск рабочего дерева

```bash
git status --short
cargo build --locked
cargo test --locked --workspace
./target/debug/lemon demo
```

Во время разработки запускайте `./target/debug/lemon`: он обновляется командой `cargo build`. Глобальная команда `lemon` обновляется отдельно:

```bash
cargo install --path crates/app --locked --force
```

Если поведение двух команд различается, сравните:

```bash
which lemon
./target/debug/lemon --version
lemon --version
```

## Полный набор проверок

```bash
cargo fmt --all --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked
python3 scripts/smoke.py
python3 scripts/report_smoke.py
```

Для небольшой правки сначала можно запустить тест изменённого crate, затем обязательно выполнить проверки workspace перед завершением задачи:

```bash
cargo test --locked -p analysis
cargo clippy --locked -p analysis --all-targets -- -D warnings
```

## Разработка плагина

Example-плагин собирается отдельно и затем подключается локальным путём:

```bash
cargo build --release --manifest-path examples/plugins/example-summary/Cargo.toml
cargo run --locked -p app -- plugins add examples/plugins/example-summary
cargo run --locked -p app -- plugins inspect example-summary
cargo run --locked -p app -- plugin run example-summary \
  --input /absolute/path/to/analysis \
  --config examples/plugins/example-summary/config.json \
  --output /tmp/lemon-example-plugin-output
```

LEMON не собирает и не запускает плагин при `plugins add`, старте приложения или открытии вкладки Plugins. Сборка — явная ответственность разработчика.

`smoke.py` проверяет TUI в PTY, запись, метки, паузу, Ctrl+C и replay. Rust-тесты покрывают декодеры, DSP, storage, DMOV, report, Plugin Platform и Settings. Обязательные тесты используют только синтетические fixture и не читают реальные записи из `Downloads`.

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
lemon plugins --help
lemon plugin run --help
```
