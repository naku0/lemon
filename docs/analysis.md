# Технический анализ

`lemon run analyze` превращает запись в открытый набор JSON и CSV. Этот набор называется **AnalysisBundle**. Его можно читать вручную, открыть в Python или передать генератору отчётов и плагинам.

## Быстрый пример

```bash
lemon run analyze recording.csv --output reports/recording-analysis
```

Для DAT без надёжной частоты:

```bash
lemon run analyze recording.dat \
  --sample-rate 250 \
  --output reports/recording-analysis
```

Для уже записанного сеанса LEMON:

```bash
lemon run analyze recordings/session-name \
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
  excluded-windows.csv
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
| `excluded-windows.csv` | спектральные кандидаты, исключённые из PSD, и машиночитаемые причины |

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
lemon run report create \
  --analysis reports/recording-analysis \
  --output reports/recording-report
```

Или запустите аналитический плагин:

```bash
lemon run plugin PLUGIN_ID \
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

## Предварительная обработка: Bundle v2 / DSP v3

Новые bundle имеют `analysis_format_version: 2` и `dsp_algorithm_version: 3`. Один и тот же причинный путь используется в live, replay и analyze:

```text
Raw → проверка порядка и чисел → артефакты → causal filters
    → filter warmup → непрерывный segment → eligible window
    → centering → symmetric Hann → one-sided PSD → band power
```

Значения с `InvalidPacket`, `LostSample`, `Outlier`, `Saturation` или `Disconnected` остаются в колонке `raw`, но получают пустой `filtered`, `sample_valid=false` и начинают новый `segment_id`. Нули вместо пропусков не создаются.

После старта и каждого сброса фильтры прогреваются. По умолчанию это 2 секунды. Во время прогрева Filtered доступен для просмотра, но строка имеет `filter_warmup=true`, флаг `FilterWarmup` и не попадает в спектральное окно.

`processed.csv` дополнительно содержит `segment_id`, `sample_valid`, `filter_warmup` и `artifact_rejected`. `spectrum.csv` и `band-power.csv` содержат только пригодные окна одного сегмента. Причины исключения перечислены в `excluded-windows.csv`.

PSD использует вычитание среднего, symmetric Hann и one-sided нормировку в `signal_units²/Hz`. Мощность диапазона — сумма PSD bins с умножением на `Δf` для полуоткрытой границы `low ≤ f < high`. Относительная мощность делится на всю non-DC PSD.

Bundle v1 остаётся читаемым и считается `dsp_algorithm_version: 1`; прежние bundle v2 с DSP v2 также читаются. Отсутствующий `windowing_algorithm_version` означает старую сетку (0). Новые результаты используют DSP v3 и windowing v1 при неизменном файловом формате v2. Для строгого сравнения с новыми результатами исходную запись следует повторно обработать командой `lemon run analyze`.

### Настройки и учёт окон

Старые JSON-конфигурации получают эти значения автоматически:

```json
{
  "artifact_policy": "reject_and_reset",
  "filter_warmup_seconds": 2.0,
  "window_function": "hann_symmetric",
  "reject_low_quality_windows": true
}
```

Прогрев занимает `ceil(filter_warmup_seconds × sample_rate_hz)` подряд пригодных отсчётов. Отвергнутый выброс не меняет последнюю пригодную опорную точку (`ArtifactRejected`). При потере непрерывности, InvalidPacket, отключении, смене источника или явном reset опорная точка очищается (`ContinuityBreak`): новый корректный уровень не сравнивается со старым. Saturation проверяется в обоих случаях. Фильтры и EMA сбрасываются независимо по каналам. Недостаточная синхронизация каналов запрещает асимметрию, но сама по себе не исключает одноканальный PSD.

Внутри Processor входные блоки проходят одинаковую последовательность шагов. `process_with_frames` отдаёт каждое hop-обновление через callback, без накопления спектров. `process` оставляет последний снимок для интерфейса. Анализ получает Filtered шагом по одному отсчёту и передаёт его общему генератору. Только принятые кандидаты поступают в общий PSD API; собственных правил шага или исключения в анализе нет.

`excluded-windows.csv` содержит:

```csv
channel,segment_id,window_index,window_start_ns,window_end_ns,reason_codes,details
```

Сетку задаёт общий `processing::windows::WindowGenerator`, используемый анализом и Mu-плагином. Временной шаг проверяется с допуском `0.51` периода. Причины задаются enum `WindowRejectionReason`; повторяющиеся причины удаляются, порядок кодов стабилен.

Сетка кандидатов начинается заново при смене сегмента или переходе между пригодными и непригодными отсчётами. Полные кандидаты имеют размер `window_samples` и шаг `hop_samples`. На границе каждого такого участка и в конце файла записывается один незавершённый кандидат, если после последнего полного окна поступили новые отсчёты. Его временные границы относятся только к реально имеющимся данным. Кандидаты не достраиваются нулями.

Причины разделены `|`; например, `FilterWarmup|InsufficientData`. Индекс общий для пригодных и исключённых кандидатов внутри канала. Счётчики причин могут суммарно превышать число исключённых окон: у одного окна бывает несколько причин. Эти числа описывают принятую сетку, а не количество потерянных отсчётов.

Filtered-статистика включает конечные значения прогрева, доступные в CSV; спектральная статистика использует только `sample_valid=true`. Качество — инженерная эвристика: проверка плоского сигнала требует накопленных отсчётов, а доля мощности около notch оценивается по полному окну. История этих решений не зависит от размера переданного блока. Оценка качества имеет отдельный ограниченный буфер: исключение FFT-окна не стирает признаки плоского сигнала или сетевой составляющей. После сброса до накопления `min(32, window_samples)` пригодных post-warmup отсчётов не выставляется `Good`: при отсутствии явных ошибок показывается `Unknown`.

Пример provenance нового manifest (фрагмент):

```json
{
  "analysis_format_version": 2,
  "dsp_algorithm_version": 3,
  "windowing_algorithm_version": 1,
  "psd_algorithm_version": 2,
  "processing_mode": "causal_live_compatible",
  "window_function": "hann_symmetric",
  "filter_warmup_seconds": 2.0,
  "artifact_policy": "reject_and_reset",
  "reject_low_quality_windows": true
}
```

Причинные IIR-фильтры имеют фазовую задержку; это не zero-phase обработка. Фиксированные 2 секунды прогрева не гарантируют одинаковую точность установления для любой конфигурации. Вычитание среднего выполняется **до** Hann: у произвольного конечного сигнала после взвешивания DC не обязан быть строго нулевым. При сравнении результатов сохраняйте параметры фильтров, прогрева, качества и версию DSP.

## Если output уже существует

Выберите новый каталог или явно замените только указанный результат:

```bash
lemon run analyze INPUT --output reports/analysis-2
lemon run analyze INPUT --output reports/analysis --overwrite
```

Полный список опций:

```bash
lemon run analyze --help
```

Если native session содержит `experiment.json`, анализ проверяет, копирует его без изменения в AnalysisBundle и перечисляет в `manifest.output_files`. Так будущие event-locked плагины получают resolved plan и фактические markers, не разбирая внутреннее состояние TUI.
