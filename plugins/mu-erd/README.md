# Event-locked Mu-band power analysis

`mu-erd` сравнивает Mu-мощность каждого trial с его собственным baseline внутри одной записи. Дополнительно показывает Beta. Это описательный анализ мощности, без физиологической интерпретации и тестов статистической значимости.

`mu-condition-analysis` сравнивает **отдельные записи** упражнений с фоном. `mu-erd` использует **маркеры внутри одной записи**: baseline, cue, TaskStart, TaskEnd и номер trial.

## 1. Подготовьте запись

Запустите эксперимент с выбранной конфигурацией источника и завершите protocol:

```bash
lemon run experiment \
  --protocol examples/motor-imagery-demo.json \
  --config examples/serial-tagged-u8-a0.json \
  --port /dev/cu.usbmodem1401 --record
```

Это пример одноканального подключения A0. Порт выбирайте по `lemon ports`; для двух каналов заранее настройте выбранные channel tags. В конфигурации плагина перечисляйте реальные имена каналов из manifest.

Для проверки без оборудования:

```bash
lemon run experiment \
  --protocol examples/motor-imagery-demo.json \
  --demo --headless --record
```

Demo создаёт каналы `ch1`/`ch2` согласно acquisition config: замените `channels` в копии `examples/mu-erd.json`. Не ускоряйте protocol через `--time-scale` для этого примера: при сокращении фаз двухсекундные окна перестанут помещаться в baseline/task.

Команда напечатает путь записи. Подставьте его вместо `recordings/SESSION`:

```bash
lemon run analyze recordings/SESSION --output reports/session-analysis
```

Нужны AnalysisBundle v2, DSP v3, PSD v2, windowing v1, causal processing и `hann_symmetric`. Плагин проверит manifest, `processed.csv`, `excluded-windows.csv` и `experiment.json`. Старые bundle нужно заново получить через `lemon run analyze`.

## 2. Соберите и подключите один раз

Из корня репозитория:

```bash
cargo build --locked --release -p mu-erd --target-dir plugins/mu-erd/target
lemon plugins add plugins/mu-erd
lemon plugins inspect mu-erd
```

`--target-dir` важен: исполняемый файл должен находиться **внутри каталога плагина**, как указано в `plugin.json`. На Windows используйте имя `target/release/mu-erd.exe` в manifest. LEMON не собирает плагин автоматически.

## 3. Настройте интервалы и запустите

Скопируйте [examples/mu-erd.json](../../examples/mu-erd.json), укажите доступные каналы и conditions из resolved plan, затем:

```bash
lemon run plugin mu-erd \
  --input reports/session-analysis \
  --config examples/mu-erd.json \
  --output reports/session-mu-erd
```

Output должен быть пустым. Чтобы сознательно заменить предыдущий результат, используйте CLI-флаг `--overwrite`.

## Как устроена конфигурация

Все времена заданы в секундах относительно `TaskStart`; первая версия поддерживает только этот anchor.

| Поле | Смысл |
|---|---|
| `channels`, `conditions` | Непустые списки уникальных имён, присутствующих во входе |
| `epoch_seconds` | Общий интервал извлечения, правая граница исключается |
| `baseline_seconds` | Интервал до anchor, целиком внутри фактических BaselineStart–BaselineEnd |
| `analysis_seconds` | Интервал после anchor, целиком внутри фактических TaskStart–TaskEnd |
| `window_seconds`, `hop_seconds` | Размер/шаг; должны давать целое число отсчётов, окно 8–65536 |
| `minimum_baseline_windows`, `minimum_analysis_windows` | Минимальное количество полных пригодных окон |
| `bands` | По умолчанию Mu 8–13 и Beta 13–30 Гц; границы `[low, high)`, без пересечения, ниже Nyquist |
| `allow_cancelled` | По умолчанию `false`; явный `true` допускает завершённые trials отменённого эксперимента |

В demo protocol baseline длится 4 с, затем идёт cue 1 с и task 4 с. Поэтому baseline находится приблизительно в `[-5,-1]` относительно TaskStart. Пример использует epoch `[-4,4]` и baseline `[-4,-1]`: три полных окна длиной 2 с с шагом 0.5 с. Если фактический cue длился больше секунды, сдвиньте правую границу baseline раньше (например, `-1.5` оставляет два окна). Плагин проверяет реальные markers и не подменяет их запланированными временами.

Baseline `[-2,0]` при двухсекундном окне содержит только **одно** полное окно и захватывает cue. Он не подходит для minimum=2. Окна, пересекающие границы baseline/task, не используются: task никогда не дополняет baseline.

## Что считается

1. Проверяются ID protocol, trial/condition, порядок/уникальность и полнота фаз.
2. Из Filtered выбирается epoch. Raw и базовые фильтры не пересчитываются.
3. Общий `processing::windows::WindowGenerator` создаёт кандидаты; общий PSD API считает mean-centered symmetric Hann periodogram, интеграция — `sum(PSD * df)`.
4. Для каждого trial/channel/band baseline — медиана мощности полных baseline windows.
5. Изменение: `100*(P/B-1)` процентов и `10*log10(P/B)` dB.
6. Медиана, Q1 и Q3 агрегируются по отдельным trials. Квантили линейные (type 7). Временные окна объединяются только при **точно одинаковом смещении начала относительно anchor** в наносекундах. Различающиеся сетки не интерполируются.

Версия 1 консервативна: любой artifact, gap, смена segment, warmup или LowQuality в epoch исключает весь trial для этого канала. Пригодность берётся из v2 `sample_valid`, флагов и timestamped quality events; запаздывающая display-quality не пересчитывается. Неполный хвост генератора сам по себе не исключает trial, если полных окон достаточно. Paused и неполные trials исключаются. Отрицательный baseline/нулевой baseline не нормализуется; для нулевой analysis power результат -100%, поле dB пустое.

## Что открыть в результате

| Файл | Содержимое |
|---|---|
| `summary.json` | Версии, хеши всех входных файлов, protocol hash, параметры DSP/окон, причины исключений и warnings |
| `trial-results.csv` | Одна строка на trial/channel/band; включён ли trial и почему исключён |
| `windowed-power.csv` | Полные baseline/analysis windows, абсолютная мощность, baseline и нормализация |
| `excluded-trials.csv` | Причины исключения trial/channel |
| `rejected-windows.csv` | Исходные причины отклонения кандидатов общего генератора |
| `timecourse.csv` | Медиана/Q1/Q3 по trials, отдельно для condition/channel/band/времени |
| `condition-summary.csv` | Медиана/Q1/Q3 per-trial изменения по условию и каналу |
| `figures/` | Mu/Beta timecourse для каждого канала и отдельные condition summary |
| `plugin-result.json` | Стандартный список реально созданных артефактов для LEMON |

У исключённых trials нормализованные значения пусты и `included=false`; они не участвуют в графиках. Абсолютные мощности доступных окон сохраняются для проверки. Даже пустые CSV имеют заголовки. Если пригодных trials нет, создаются таблицы/JSON с объяснением, без пустых PNG.

`included_trials` в summary означает число trials с хотя бы одним пригодным каналом. `fully_included_trials` требует все выбранные каналы; `per_channel_counts` показывает частичные исключения. Число N на каждой точке — количество разных trials, а не перекрывающихся окон. Исходные окна базового анализа и окна плагина могут иметь другую сетку; их счётчики исключений хранятся отдельно.

PNG содержат версии DSP/PSD/windowing. В CSV версии есть в каждой строке. Не-ASCII подписи на простом встроенном растровом шрифте показываются кодами Unicode, полные исходные имена остаются в CSV/JSON; для необычных имён файлы используют UTF-8 hex. Python для запуска не требуется.

## Ограничения

- Arduino/Serial markers с `last_observed_sample` привязаны к последнему известному отсчёту. Это не аппаратные triggers; субмиллисекундная синхронизация стимула и EEG не гарантируется.
- Даже completed trial может быть исключён: интервалы должны помещаться в реально записанные фазы.
- A0/A2 не назначаются C3/C4. Нет выводов о полушариях, намерении или состоянии человека, нет межканальной фазы/когерентности/асимметрии.
- Это описательное изменение мощности, без клинического утверждения MuERD/ERS и без p-value. Перекрывающиеся окна коррелированы.
- Один bundle на запуск. Выбранные Filtered хранятся в памяти, лимит 10 миллионов отсчётов. JSON ограничен 4 MiB. Большие записи нужно делить на самостоятельные эксперименты.

Следующая отдельная задача — калибровочный rule-based control plugin: сначала offline калибровка порогов и проверка на отложенных trials, затем явные правила безопасного включения. `mu-erd` не управляет устройствами.
