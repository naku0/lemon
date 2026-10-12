# Технический анализ

`lemon analyze` превращает запись в открытый набор JSON и CSV. Этот набор называется **AnalysisBundle**. Его можно читать вручную, открыть в Python или передать генератору отчётов и плагинам.

## Быстрый пример

```bash
lemon analyze recording.csv --output reports/recording-analysis
```

Для DAT без надёжной частоты:

```bash
lemon analyze recording.dat \
  --sample-rate 250 \
  --output reports/recording-analysis
```

Для уже записанного сеанса LEMON:

```bash
lemon analyze recordings/session-name \
  --output reports/session-analysis
```

Если команда завершилась успешно, она напечатает путь к bundle.

## Что появится в каталоге

```text
reports/recording-analysis/
  manifest.json
  summary.json
  summary.md
  processed.csv
  spectrum.csv
  band-power.csv
  quality-events.csv
```

Начните с двух файлов:

```bash
cat reports/recording-analysis/summary.md
python3 -m json.tool reports/recording-analysis/manifest.json | head -80
```

| Файл | Что в нём искать |
|---|---|
| `manifest.json` | источник, hash, каналы, единицы, частота и фактическая конфигурация обработки |
| `summary.json` | структурированные итоговые показатели для программ и плагинов |
| `summary.md` | короткое техническое резюме для человека |
| `processed.csv` | Raw и Filtered по каждому отсчёту и каналу |
| `spectrum.csv` | все готовые PSD-окна и частотные bins |
| `band-power.csv` | absolute, relative, smoothed и baseline change по окнам |
| `quality-events.csv` | пропуски, разрывы и события инженерной quality-оценки |

## Посмотреть данные без notebook

Первые строки Filtered и Raw:

```bash
head reports/recording-analysis/processed.csv
```

Число строк по файлам:

```bash
wc -l reports/recording-analysis/*.csv
```

Для удобного просмотра больших CSV используйте Python, R, Julia, MATLAB или табличный редактор. `processed.csv` имеет long-form: одна строка соответствует одному отсчёту одного канала.

## Создать отчёт из готового bundle

Повторный анализ не нужен:

```bash
lemon report \
  --analysis reports/recording-analysis \
  --output reports/recording-report
```

Или запустите аналитический плагин:

```bash
lemon plugin run PLUGIN_ID \
  --input reports/recording-analysis \
  --config plugin-config.json \
  --output reports/plugin-result
```

## Важные свойства

- используется тот же Rust DSP, что в live/replay;
- Raw и Filtered сохраняются раздельно;
- пропуски остаются пропусками;
- спектральные окна не теряются при потоковой обработке;
- одинаковые границы Alpha/Mu не получают два независимых «голоса»;
- summary описывает числа и обработку, не состояние человека.

## Если output уже существует

Выберите новый каталог или явно замените только указанный результат:

```bash
lemon analyze INPUT --output reports/analysis-2
lemon analyze INPUT --output reports/analysis --overwrite
```

Полный список опций:

```bash
lemon analyze --help
```
