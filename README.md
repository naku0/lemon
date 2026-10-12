# LEMON

LEMON — Lightweight EEG Monitoring Of Neuroactivity — локальное Rust-приложение для приёма, записи, обработки и технического анализа сигналов.

Приложение поддерживает синтетический demo-режим, Serial, replay, CSV/DAT/DMOV import, потоковый `analyze` и воспроизводимые Jupyter-отчёты. LEMON не является медицинским изделием: quality — инженерная эвристика, а результаты не содержат медицинской, когнитивной или психологической интерпретации.

## Возможности

- TUI: Sources, Waveform, Spectrum, Bands, Recording и Settings.
- Demo без оборудования.
- Serial CSV и бинарный `tagged-u8`.
- Replay и запись Raw до DSP.
- Прямой DMOV-профиль для двух каналов Лаб2.
- AnalysisBundle через `lemon analyze`.
- Local notebook, portable ZIP и Colab через `lemon report`.

Подробности находятся в [docs/](docs/): [архитектура](docs/architecture.md), [Serial](docs/serial.md), [DMOV](docs/dmov.md), [анализ](docs/analysis.md), [отчёты](docs/reports.md), [Settings](docs/settings.md) и [разработка](docs/development.md). Полная прежняя документация сохранена в [docs/reference.md](docs/reference.md).

## Требования

- стабильный Rust и Cargo;
- интерактивный терминал для TUI;
- Python 3.10+ для выполнения локальных отчётов;
- разрешение на Serial-порт при работе с оборудованием.

Первая сборка Cargo может обращаться к crates.io. Обычная запись, replay, analyze и `report --no-execute` сеть не используют.

## Сборка и установка

```bash
cd /Users/sniyaq/diploma/lemon
cargo build --locked
cargo test --locked
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Запуск собранного бинарника:

```bash
./target/debug/lemon --help
```

Установка глобальной команды:

```bash
cargo install --path crates/app --locked --force
lemon --version
```

Если macOS выдаёт `Permission denied` для системной временной папки, проверьте `TMPDIR`:

```bash
unset TMPDIR
cargo build --locked
```

## Быстрый старт

```bash
lemon demo
lemon demo --headless --duration 10 --record
lemon replay recordings/session-name
lemon ports
```

По умолчанию записи создаются в `recordings/` в текущем рабочем каталоге. Путь печатается при завершении.

## Основные команды

```bash
lemon demo [--channels 1|2] [--record]
lemon live --port PORT --baud 115200 --protocol csv --channels 1
lemon live --config examples/serial-tagged-u8-a0.json --port PORT --record
lemon probe --port PORT --baud 115200 --duration 10 --output capture.bin
lemon replay INPUT [--headless] [--duration SECONDS]
lemon import INPUT --output recordings/session
lemon analyze INPUT --output reports/session-analysis
lemon report INPUT
```

Arduino UNO:

```bash
lemon live \
  --config examples/serial-tagged-u8-a0.json \
  --port /dev/cu.usbmodem1401 \
  --channels 1 \
  --record
```

Не открывайте этот порт в Arduino Serial Monitor одновременно. Открытие порта может перезапустить UNO.

Лаб2:

```bash
lemon import \
  "$HOME/Downloads/Лаб2/фон/5_10_2026_12_17_26.dmov" \
  --output recordings/lab2-background
```

## Конфигурация

- `config.json` — источник, частота, DSP, диапазоны и запись;
- `settings.json` — тема, цвета, отображение и личные предпочтения.

```bash
lemon config init examples/local.json
lemon config check examples/local.json
lemon config init --user
```

Приоритет: CLI → environment → settings → experiment config → defaults.

## Структура проекта

```text
crates/core/       доменные типы и конфигурация
crates/source/     synthetic, replay, Serial, decoders и probe
crates/processing/ DSP, фильтры, PSD, bands и quality
crates/storage/    native session и внешние форматы
crates/analysis/   AnalysisBundle и статистика
crates/report/     notebook, Python environment, ZIP и Colab
crates/tui/        экраны, Settings и темы
crates/app/        CLI и жизненный цикл
```

## Ограничения

- до двух каналов в текущем live pipeline;
- Arduino `tagged-u8` хранится в `adc_counts_u8`, без физической калибровки;
- DMOV не доказывает синхронность отдельных каналов;
- C3/C4 назначает пользователь;
- quality не является клинической метрикой;
- автоматического reconnect нет.

## Проверки

```bash
cargo fmt --all --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo build --locked
python3 scripts/smoke.py
python3 scripts/report_smoke.py
```

## Лицензия

MIT.
