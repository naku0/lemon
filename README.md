# LEMON

LEMON — локальное приложение для записи, просмотра и технического анализа EEG-сигналов. Оно работает с Arduino Serial, CSV, DAT, DMOV и собственными записями LEMON.

Если вы открыли проект впервые, начните с [пошагового руководства](docs/getting-started.md). Оно проводит от сборки до первого отчёта без подключения оборудования.

> LEMON показывает сигнал и численные характеристики обработки. Он не ставит диагнозы и не делает выводов о состоянии, эмоциях или намерениях человека.

## Что умеет LEMON

- показывает Raw, Filtered, spectrum, band power и quality в терминальном интерфейсе;
- записывает исходные отсчёты до DSP и воспроизводит сохранённые сеансы;
- принимает Serial CSV и бинарный tagged-u8 от Arduino;
- импортирует CSV, DAT, DMOV и каталоги внешних записей;
- пакетно подготавливает отдельные фоновые и condition-записи через offline study;
- создаёт AnalysisBundle с JSON/CSV и нейтральным техническим summary;
- генерирует локальные Jupyter-отчёты, portable ZIP и Colab notebook;
- запускает локальные аналитические плагины в отдельных процессах.
- выполняет воспроизводимые trial-протоколы и записывает структурированные маркеры.

## Что вы хотите сделать?

| Задача | Первая команда | Инструкция |
|---|---|---|
| Посмотреть интерфейс без оборудования | `lemon run demo` | [Быстрый старт](docs/getting-started.md) |
| Записать сигнал с Arduino | `lemon ports` | [Serial и Arduino](docs/serial.md) |
| Открыть старую запись | `lemon run replay PATH` | [Быстрый старт](docs/getting-started.md#4-откройте-готовую-запись) |
| Импортировать DMOV Лаб‑2 | `lemon import FILE.dmov` | [DMOV](docs/dmov.md) |
| Получить CSV и численное резюме | `lemon run analyze INPUT` | [AnalysisBundle](docs/analysis.md) |
| Подготовить фон и упражнения Лаб‑2 | `lemon run study --help` | [Offline study](docs/studies.md) |
| Создать Jupyter Notebook | `lemon run report create INPUT` | [Отчёты](docs/reports.md) |
| Запустить аналитический плагин | `lemon plugins list` | [Плагины](docs/plugins.md) |
| Записать эксперимент с фазами | `lemon run experiment --help` | [Эксперименты](docs/experiments.md) |
| Исправить ошибку запуска | — | [Частые проблемы](docs/troubleshooting.md) |

Основные группы команд:

| Группа | Для чего |
|---|---|
| `lemon run …` | Запуск live, demo, replay, анализа, study, эксперимента, плагина или отчёта |
| `lemon plugins …` | Подключение и управление локальными плагинами |
| `lemon config …` | Создание и проверка конфигурации |
| `lemon import`, `export`, `ports`, `probe` | Импорт, экспорт и диагностика источников |

## Установка

Нужен стабильный Rust toolchain. Из корня репозитория:

```bash
cargo build --locked
./target/debug/lemon --version
./target/debug/lemon run demo
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
lemon run demo

# 2. Создать десятисекундную тестовую запись
lemon run demo --headless --duration 10 --record

# 3. Посмотреть созданные записи
ls recordings

# 4. Проанализировать нужный каталог, напечатанный предыдущей командой
lemon run analyze recordings/SESSION --output reports/demo-analysis

# 5. Создать notebook без настройки Python
lemon run report create --analysis reports/demo-analysis --no-execute \
  --output reports/demo-report
```

`SESSION` замените именем каталога, которое LEMON напечатал после записи. В результате появятся проверяемые CSV в `reports/demo-analysis/` и notebook `reports/demo-report/report.ipynb`.

## Типичный рабочий процесс

```text
устройство или файл
        ↓
native-запись LEMON / импорт
        ↓
lemon run analyze → AnalysisBundle с CSV и summary
        ↓
lemon run report create → Jupyter Notebook
        └───────────────→ lemon run plugin
```

Raw сохраняется отдельно от Filtered. Пропуски не заменяются нулями. Отчёты и плагины используют готовый AnalysisBundle и не меняют исходную запись.

## Где искать результат

- `run live`/`run demo` с `--record`: каталог `recordings/` в текущей директории;
- `run analyze --output DIR`: указанный `DIR`;
- `run report create --output DIR`: `DIR/report.ipynb`;
- `run plugin --output DIR`: `DIR/plugin-result.json` и заявленные плагином файлы.

LEMON также печатает итоговый путь после успешной команды. Существующие результаты не перезаписываются без явного `--overwrite`.

## Документация

Полный указатель находится в [`docs/README.md`](docs/README.md).

- [Быстрый старт](docs/getting-started.md) — установка и полный учебный сценарий;
- [Serial и Arduino](docs/serial.md) — поиск порта, `probe`, один и два канала;
- [DMOV](docs/dmov.md) — импорт записей Лаб‑2;
- [Технический анализ](docs/analysis.md) — содержимое AnalysisBundle;
- [Offline study](docs/studies.md) — единая подготовка фона и отдельных условий;
- [Jupyter-отчёты](docs/reports.md) — Python environment, local, ZIP и Colab;
- [Плагины](docs/plugins.md) — подключение и запуск, включая Mu condition analysis и [event-locked Mu/Beta](plugins/mu-erd/README.md);
- [Эксперименты](docs/experiments.md) — protocol JSON, trials, markers и sidecar;
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
