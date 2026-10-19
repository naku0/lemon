# Разработка

## Первый запуск рабочего дерева

```bash
git status --short
cargo build --locked
cargo test --locked --workspace
./target/debug/lemon run demo
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

После изменения DSP отдельно проверьте публичную спектральную математику и формат bundle:

```bash
cargo test --locked -p processing
cargo test --locked -p analysis
cargo test --locked -p lemon-plugin-sdk
cargo test --locked -p mu-condition-analysis
```

Тесты должны покрывать артефакты до фильтров, warmup после старта и сброса, segment ID, PSD-нормировку и чтение старого Bundle v1.

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
cargo run --locked -p app -- run plugin example-summary \
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
lemon run --help
lemon run live --help
lemon probe --help
lemon run analyze --help
lemon run report --help
lemon plugins --help
lemon run plugin --help
lemon run experiment --help
```

## CLI migration

Старые формы остаются совместимыми и проходят через те же внутренние запросы.
Они скрыты из обычного help и пока не печатают предупреждений:

| Legacy | Canonical |
|---|---|
| `lemon live …` | `lemon run live …` |
| `lemon demo …` | `lemon run demo …` |
| `lemon replay INPUT …` | `lemon run replay INPUT …` |
| `lemon analyze INPUT …` | `lemon run analyze INPUT …` |
| `lemon experiment run …` | `lemon run experiment …` |
| `lemon plugin run …` | `lemon run plugin …` |
| `lemon report INPUT …` | `lemon run report create INPUT …` |
| `lemon report setup …` | `lemon run report setup …` |
| `lemon report doctor` | `lemon run report healthcheck` |

Offline study подключён вариантом `RunAction::Study`. Его orchestration и атомарная
публикация находятся в crate `study`, а CLI только передаёт manifest, эффективный DSP
config и output. Детерминированные тесты используют синтетические CSV, DAT и DMOV:

```bash
cargo test --locked -p study
./target/debug/lemon run study --help
```

Experiment engine тестируется синтетически в crate `experiment`. Ускоренный end-to-end сценарий не требует оборудования:

```bash
cargo run --locked -p app -- run experiment \
  --protocol examples/motor-imagery-demo.json \
  --demo --headless --time-scale 0.01 \
  --data-dir /tmp/lemon-experiment-check
```

Регрессия DSP v3 проверяет восстановление нового уровня после пропуска, отключения, InvalidPacket, смены источника и явного reset. Сквозной тест Mu-плагина создаёт синтетическую native session блоками 1/37/800 отсчётов: после выброса вне hop-сетки, прогрева и разрыва `analyze` и плагин должны совпасть по индексам, границам, segment ID, причинам исключения и мощности. Реальные записи не нужны.
