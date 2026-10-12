# Jupyter-отчёты

`lemon report` создаёт notebook с таблицами и графиками. Он использует готовый AnalysisBundle и не выполняет фильтрацию, FFT или расчёт band power заново.

## Самый простой вариант

```bash
lemon report recording.csv --output reports/recording
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
lemon report setup
lemon report doctor
```

`setup` создаёт отдельный venv LEMON в каталоге данных ОС и устанавливает зависимости отчётов. Системный Python и окружение проекта не изменяются.

Успешный `doctor` показывает:

- путь к Python внутри venv;
- версии `numpy`, `pandas`, `matplotlib`, `nbformat`, `nbclient` и `ipykernel`;
- результат выполнения тестового notebook.

Если окружение повреждено:

```bash
lemon report setup --recreate
```

## 2. Создайте локальный выполненный отчёт

Из исходной записи:

```bash
lemon report recording.dat \
  --sample-rate 250 \
  --output reports/recording
```

Из уже готового AnalysisBundle:

```bash
lemon report \
  --analysis reports/recording-analysis \
  --output reports/recording
```

Открыть notebook после создания:

```bash
lemon report INPUT --output reports/recording --open
```

Без `--open` GUI не запускается.

## Notebook без Python

Чтобы только сгенерировать `.ipynb`:

```bash
lemon report INPUT \
  --no-execute \
  --output reports/recording
```

Все Markdown и Code cells будут на месте, outputs останутся пустыми. Этот режим подходит для переноса в другое окружение.

## Переносимый ZIP

```bash
lemon report INPUT --portable --output reports/recording
```

Рядом появится `.lemon-report.zip` с notebook, полным AnalysisBundle, PNG, manifest, requirements и README. После распаковки относительные пути продолжают работать.

## Один notebook для Google Colab

```bash
lemon report INPUT --colab --output reports/recording-colab
```

LEMON встраивает компактный Report Dataset в `.ipynb`. Для стандартного просмотра отдельные CSV загружать не нужно.

Попытаться добавить полный sample-level Raw:

```bash
lemon report INPUT --colab --include-raw \
  --output reports/recording-colab-raw
```

Если лимит размера превышен, LEMON создаст отдельный data ZIP и upload cell вместо огромного notebook. Это будет явно записано в manifest.

## Работа в VS Code или Jupyter

Откройте `report.ipynb` и выберите kernel из управляемого окружения, путь к которому показывает:

```bash
lemon report doctor
```

Если редактор выбрал `/usr/local/bin/python3`, может появиться `ModuleNotFoundError`, даже когда `doctor` успешен: это другой Python.

## Где смотреть статус

Откройте `report-manifest.json`. Поле `execution_status` различает выполненный notebook, режим без выполнения и ошибку. При ошибке исходный сгенерированный notebook сохраняется.

## Частые команды

```bash
lemon report --help
lemon report doctor
lemon report setup --recreate
lemon report --analysis ANALYSIS --no-execute --output REPORT
```

Дополнительные решения находятся в [troubleshooting.md](troubleshooting.md#notebook-не-выполняется).
