# Быстрый старт

Это руководство позволяет проверить весь путь LEMON без Arduino и пользовательских EEG-записей.

## 1. Соберите приложение

Откройте терминал в каталоге проекта:

```bash
cd /path/to/lemon
cargo build --locked
```

Проверьте бинарник:

```bash
./target/debug/lemon --version
```

Для команды `lemon` без полного пути установите приложение в Cargo bin:

```bash
cargo install --path crates/app --locked --force
which lemon
lemon --version
```

Обычно Cargo устанавливает бинарник в `~/.cargo/bin/lemon`. Если shell его не находит, откройте [решение для PATH](troubleshooting.md#команда-lemon-не-найдена).

## 2. Откройте demo

```bash
lemon demo
```

Откроется терминальный интерфейс с синтетическим сигналом. Оборудование и файлы для этого не нужны.

Полезные клавиши:

- `?` — справка;
- стрелки и `Tab` — навигация;
- `Space` — пауза отображения;
- `Esc` — закрыть окно или выйти;
- `Ctrl+C` — корректно завершить работу.

## 3. Создайте тестовую запись

```bash
lemon demo --headless --duration 10 --record
```

Команда работает десять секунд без TUI и напечатает путь созданного сеанса. По умолчанию он находится в `recordings/` текущего каталога:

```text
recordings/<имя-сеанса>/
  metadata.json
  raw.csv
  events.csv
```

Посмотрите доступные записи:

```bash
ls -lt recordings
```

## 4. Откройте готовую запись

Подставьте каталог, который напечатал предыдущий шаг:

```bash
lemon replay recordings/SESSION
```

Для быстрой проверки без интерфейса:

```bash
lemon replay recordings/SESSION --headless --duration 5
```

Replay не изменяет исходную запись.

## 5. Создайте AnalysisBundle

```bash
lemon analyze recordings/SESSION \
  --output reports/demo-analysis
```

Проверьте результат:

```bash
ls reports/demo-analysis
cat reports/demo-analysis/summary.md
```

В каталоге будут `manifest.json`, два summary и четыре CSV. Что находится в каждом файле, описано в [руководстве по анализу](analysis.md).

## 6. Создайте Jupyter-отчёт

Для первого раза Python не нужен:

```bash
lemon report \
  --analysis reports/demo-analysis \
  --no-execute \
  --output reports/demo-report
```

Notebook появится здесь:

```text
reports/demo-report/report.ipynb
```

Чтобы LEMON сам выполнял notebook и сохранял графики, один раз настройте окружение:

```bash
lemon report setup
lemon report doctor
```

После успешного `doctor`:

```bash
lemon report \
  --analysis reports/demo-analysis \
  --output reports/demo-report-executed
```

Подробно режимы local, portable ZIP и Colab разобраны в [reports.md](reports.md).

## 7. Посмотрите плагины

```bash
lemon plugins list
```

Пустой список при первом запуске — нормальное состояние. Плагины подключаются один раз локальным путём. Пошаговый пример для `mu-condition-analysis` находится в [plugins.md](plugins.md#mu-condition-analysis).

## Что попробовать дальше

- подключить Arduino: [Serial и Arduino](serial.md);
- импортировать DMOV Лаб‑2: [DMOV](dmov.md);
- открыть и изменить notebook: [отчёты](reports.md);
- настроить цвета и графики: [Settings](settings.md);
- устранить ошибку: [частые проблемы](troubleshooting.md).
