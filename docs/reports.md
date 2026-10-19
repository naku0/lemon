# Jupyter-отчёты

`lemon run report create` создаёт notebook с таблицами и графиками. Он использует готовый AnalysisBundle и не выполняет фильтрацию, FFT или расчёт band power заново.

## Самый простой вариант

```bash
lemon run report create recording.csv --output reports/recording
```

Команда сначала выполнит `analyze`, затем создаст и выполнит notebook через управляемое Python-окружение. Она не открывает браузер без `--open`.

Результат:

```text
reports/recording/
  report.ipynb
  report-manifest.json
  report-requirements.txt
  analysis/
  figures/
```

## 1. Настройте Python один раз

```bash
lemon run report setup
lemon run report healthcheck
```

`setup` создаёт отдельный venv LEMON в каталоге данных ОС и устанавливает зависимости отчётов. Системный Python и окружение проекта не изменяются.

Успешный `healthcheck` показывает:

- путь к Python внутри venv;
- версии `numpy`, `pandas`, `matplotlib`, `nbformat`, `nbclient` и `ipykernel`;
- результат выполнения тестового notebook.

Если окружение повреждено:

```bash
lemon run report setup --recreate
```

## 2. Создайте локальный выполненный отчёт

Из исходной записи:

```bash
lemon run report create recording.dat \
  --sample-rate 250 \
  --output reports/recording
```

Из уже готового AnalysisBundle:

```bash
lemon run report create \
  --analysis reports/recording-analysis \
  --output reports/recording
```

Открыть notebook после создания:

```bash
lemon run report create INPUT --output reports/recording --open
```

Без `--open` GUI не запускается.

## Notebook без Python

Чтобы только сгенерировать `.ipynb`:

```bash
lemon run report create INPUT \
  --no-execute \
  --output reports/recording
```

Все Markdown и Code cells будут на месте, outputs останутся пустыми. Этот режим подходит для переноса в другое окружение.

## Переносимый ZIP

```bash
lemon run report create INPUT --portable --output reports/recording
```

Рядом появится `.lemon-report.zip` с notebook, полным AnalysisBundle, PNG, manifest, requirements и README. После распаковки относительные пути продолжают работать.

## Один notebook для Google Colab

```bash
lemon run report create INPUT --colab --output reports/recording-colab
```

LEMON встраивает компактный Report Dataset в `.ipynb`. Для стандартного просмотра отдельные CSV загружать не нужно.

Попытаться добавить полный sample-level Raw:

```bash
lemon run report create INPUT --colab --include-raw \
  --output reports/recording-colab-raw
```

Если лимит размера превышен, LEMON создаст отдельный data ZIP и upload cell вместо огромного notebook. Это будет явно записано в manifest.

## Работа в VS Code или Jupyter

Откройте `report.ipynb` и выберите kernel из управляемого окружения, путь к которому показывает:

```bash
lemon run report healthcheck
```

Если редактор выбрал `/usr/local/bin/python3`, может появиться `ModuleNotFoundError`, даже когда `healthcheck` успешен: это другой Python.

## Где смотреть статус

Откройте `report-manifest.json`. Поле `execution_status` различает выполненный notebook, режим без выполнения и ошибку. При ошибке исходный сгенерированный notebook сохраняется.

## Частые команды

```bash
lemon run report --help
lemon run report healthcheck
lemon run report setup --recreate
lemon run report create --analysis ANALYSIS --no-execute --output REPORT
```

Дополнительные решения находятся в [troubleshooting.md](troubleshooting.md#notebook-не-выполняется).
