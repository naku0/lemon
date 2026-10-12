# LEMON

LEMON — локальное приложение для записи, просмотра и технического анализа EEG-сигналов. Оно работает с Arduino Serial, CSV, DAT, DMOV и собственными записями LEMON.

Если вы открыли проект впервые, начните с [пошагового руководства](docs/getting-started.md). Оно проводит от сборки до первого отчёта без подключения оборудования.

> LEMON показывает сигнал и численные характеристики обработки. Он не ставит диагнозы и не делает выводов о состоянии, эмоциях или намерениях человека.

## Что умеет LEMON

- показывает Raw, Filtered, spectrum, band power и quality в терминальном интерфейсе;
- записывает исходные отсчёты до DSP и воспроизводит сохранённые сеансы;
- принимает Serial CSV и бинарный tagged-u8 от Arduino;
- импортирует CSV, DAT, DMOV и каталоги внешних записей;
- создаёт AnalysisBundle с JSON/CSV и нейтральным техническим summary;
- генерирует локальные Jupyter-отчёты, portable ZIP и Colab notebook;
- запускает локальные аналитические плагины в отдельных процессах.

## Что вы хотите сделать?

| Задача | Первая команда | Инструкция |
|---|---|---|
| Посмотреть интерфейс без оборудования | `lemon demo` | [Быстрый старт](docs/getting-started.md) |
| Записать сигнал с Arduino | `lemon ports` | [Serial и Arduino](docs/serial.md) |
| Открыть старую запись | `lemon replay PATH` | [Быстрый старт](docs/getting-started.md#4-откройте-готовую-запись) |
| Импортировать DMOV Лаб‑2 | `lemon import FILE.dmov` | [DMOV](docs/dmov.md) |
| Получить CSV и численное резюме | `lemon analyze INPUT` | [AnalysisBundle](docs/analysis.md) |
| Создать Jupyter Notebook | `lemon report INPUT` | [Отчёты](docs/reports.md) |
| Запустить аналитический плагин | `lemon plugins list` | [Плагины](docs/plugins.md) |
| Исправить ошибку запуска | — | [Частые проблемы](docs/troubleshooting.md) |

## Установка

Нужен стабильный Rust toolchain. Из корня репозитория:

```bash
cargo build --locked
./target/debug/lemon --version
./target/debug/lemon demo
```

Чтобы команда `lemon` работала из любого каталога:

```bash
cargo install --path crates/app --locked --force
lemon --version
```

После изменений проекта повторите `cargo install ... --force`: `cargo build` обновляет только `./target/debug/lemon`, но не установленный `~/.cargo/bin/lemon`.

## Первый запуск за две минуты

```bash
# 1. Открыть демонстрационный сигнал
lemon demo

# 2. Создать десятисекундную тестовую запись
lemon demo --headless --duration 10 --record

# 3. Посмотреть созданные записи
ls recordings

# 4. Проанализировать нужный каталог, напечатанный предыдущей командой
lemon analyze recordings/SESSION --output reports/demo-analysis

# 5. Создать notebook без настройки Python
lemon report --analysis reports/demo-analysis --no-execute \
  --output reports/demo-report
```

`SESSION` замените именем каталога, которое LEMON напечатал после записи. В результате появятся проверяемые CSV в `reports/demo-analysis/` и notebook `reports/demo-report/report.ipynb`.

## Типичный рабочий процесс

```text
устройство или файл
        ↓
native-запись LEMON / импорт
        ↓
lemon analyze → AnalysisBundle с CSV и summary
        ↓
lemon report  → Jupyter Notebook
        └─────→ аналитические плагины
```

Raw сохраняется отдельно от Filtered. Пропуски не заменяются нулями. Отчёты и плагины используют готовый AnalysisBundle и не меняют исходную запись.

## Где искать результат

- live/demo с `--record`: каталог `recordings/` в текущей директории;
- `analyze --output DIR`: указанный `DIR`;
- `report --output DIR`: `DIR/report.ipynb`;
- `plugin run --output DIR`: `DIR/plugin-result.json` и заявленные плагином файлы.

LEMON также печатает итоговый путь после успешной команды. Существующие результаты не перезаписываются без явного `--overwrite`.

## Документация

Полный указатель находится в [`docs/README.md`](docs/README.md).

- [Быстрый старт](docs/getting-started.md) — установка и полный учебный сценарий;
- [Serial и Arduino](docs/serial.md) — поиск порта, `probe`, один и два канала;
- [DMOV](docs/dmov.md) — импорт записей Лаб‑2;
- [Технический анализ](docs/analysis.md) — содержимое AnalysisBundle;
- [Jupyter-отчёты](docs/reports.md) — Python environment, local, ZIP и Colab;
- [Плагины](docs/plugins.md) — подключение и запуск, включая Mu condition analysis;
- [Settings и темы](docs/settings.md) — управление интерфейсом;
- [Частые проблемы](docs/troubleshooting.md) — PATH, TMPDIR, Serial, Python и output;
- [Архитектура](docs/architecture.md) и [разработка](docs/development.md) — для разработчиков.

Прежняя расширенная справка сохранена как [архив](docs/reference.md); для обычного запуска она не нужна.

## Основные ограничения

- live pipeline поддерживает до двух выбранных каналов;
- Arduino `tagged-u8` хранит значения как `adc_counts_u8` без пересчёта в микровольты;
- DMOV не доказывает аппаратную синхронность каналов;
- соответствие A0/A2 позициям C3/C4 задаёт пользователь;
- quality является инженерной эвристикой;
- автоматического reconnect Serial пока нет.

## Лицензия

MIT.
