# Plugin Platform

Plugin Platform подключает локальные аналитические инструменты к готовому `AnalysisBundle`. Плагин работает после `lemon analyze`, в отдельном процессе, и создаёт собственные артефакты. Платформа не является частью live DSP и не содержит знаний о конкретных физиологических методиках.

## Если вы хотите просто запустить готовый плагин

Нужны три вещи:

1. собранный исполняемый файл плагина;
2. AnalysisBundle от `lemon analyze`;
3. JSON-конфигурация конкретного плагина.

На примере `example-summary`:

```bash
# 1. Собрать плагин
cargo build --release \
  --manifest-path examples/plugins/example-summary/Cargo.toml

# 2. Подключить его один раз
lemon plugins add examples/plugins/example-summary

# 3. Проверить состояние
lemon plugins list
lemon plugins inspect example-summary

# 4. Запустить
lemon plugin run example-summary \
  --input reports/demo-analysis \
  --config examples/plugins/example-summary/config.json \
  --output reports/example-plugin-result
```

После запуска откройте:

```bash
cat reports/example-plugin-result/plugin-result.json
find reports/example-plugin-result -maxdepth 2 -type f
```

Регистрация выполняется один раз. LEMON не собирает плагин при `plugins add` и не запускает его при старте приложения.

## Реестр пользователя

`lemon plugins add DIRECTORY` проверяет каталог и сохраняет только ID, канонический абсолютный путь и флаг `enabled`. Реестр `plugins.json` находится в локальном application-data каталоге LEMON, определённом через `directories::ProjectDirs`; он не входит в experiment config, AnalysisBundle или Git.

```bash
lemon plugins add ./my-plugin
lemon plugins list
lemon plugins inspect my-plugin
lemon plugins disable my-plugin
lemon plugins enable my-plugin
lemon plugins remove my-plugin
```

`remove` удаляет запись реестра, но не файлы плагина. При чтении реестра LEMON проверяет каждый актуальный `plugin.json` и показывает `ready`, `disabled`, `unavailable` или `incompatible`. Ошибка одного элемента не мешает остальным. Проверка не собирает, не загружает и не запускает код.

## Manifest

Каталог плагина содержит исполняемый файл, `plugin.json` и обычно README:

```json
{
  "schema_version": 1,
  "id": "example-summary",
  "display_name": "Example summary plugin",
  "version": "0.1.0",
  "api_version": 1,
  "description": "Minimal example LEMON analysis plugin.",
  "entrypoint": {
    "program": "target/release/example-summary",
    "args": []
  }
}
```

Версии manifest и Plugin API первой версии равны `1`. ID содержит только строчные ASCII-буквы, цифры и дефисы. `program` — относительный путь к обычному исполняемому файлу внутри каталога плагина. Абсолютные пути, `..` и симлинки наружу отклоняются.

## Запуск и модель безопасности

```bash
lemon plugin run example-summary \
  --input /absolute/path/to/analysis \
  --config ./plugin-config.json \
  --output ./plugin-output \
  --timeout 300
```

Вместо ID допустим путь к незарегистрированному каталогу. LEMON проверяет manifest, bundle и JSON-конфигурацию, подготавливает пустой output и запускает entrypoint напрямую через системный process API без shell. Рабочий каталог равен каталогу плагина. Аргументы имеют фиксированный вид:

```text
--input ABSOLUTE_ANALYSIS_BUNDLE_PATH
--config ABSOLUTE_CONFIG_PATH
--output ABSOLUTE_OUTPUT_PATH
```

Окружение процесса очищается; передаётся `LEMON_PLUGIN_API=1`. Токены, proxy и пользовательские переменные не наследуются. Stdout и stderr сохраняются в диагностические файлы output. Timeout завершает дочерний процесс. LEMON сравнивает содержимое файлов bundle до и после запуска и сообщает о модификации.

Если output непустой, запуск отклоняется. `--overwrite` очищает только точно переданный output; симлинк в качестве output запрещён. Эта версия не является системной песочницей: локальный исполняемый файл обладает правами запустившего его пользователя. Подключайте только доверенный код. Изоляция процесса, очищенное окружение и файловые проверки уменьшают случайный ущерб, но не заменяют OS sandbox.

## Результат

Плагин обязан создать `plugin-result.json`:

```json
{
  "schema_version": 1,
  "plugin": { "id": "example-summary", "version": "0.1.0" },
  "status": "completed",
  "artifacts": [
    { "path": "summary.json", "kind": "data", "description": "Example output" }
  ],
  "warnings": []
}
```

Статусы: `completed`, `completed_with_warnings`, `failed`. Виды артефактов: `data`, `table`, `figure`, `report`. ID и версия обязаны совпадать с manifest. Каждый артефакт должен существовать как файл внутри output; абсолютные пути, traversal и симлинки наружу отклоняются. Для `failed` требуется безопасное поле `error`.

## Rust SDK

`lemon-plugin-sdk` открывает bundle через единую проверку `analysis::AnalysisBundle::open`, предоставляет типизированные manifest/summary и потоковые итераторы:

```rust
use lemon_plugin_sdk::Bundle;

let bundle = Bundle::open(input)?;
println!("{:?} @ {} Hz", bundle.channels(), bundle.sample_rate_hz());
for row in bundle.processed()? {
    let row = row?;
    // row.raw and row.filtered are existing analysis values.
}
```

Доступны `processed`, `spectrum`, `band_power`, `quality_events`, `excluded_windows`, а также типы `PluginResult`, `Artifact` и атомарная `write_plugin_result`. Для Bundle v2 строки `processed` содержат `segment_id`, `sample_valid`, `filter_warmup` и `artifact_rejected`. SDK не зависит от TUI и не раскрывает состояние DSP.

Плагин, которому нужна новая спектральная оценка поверх пригодного Filtered, должен использовать публичные `processing::one_sided_psd` и `processing::integrate_band_power`. Это сохраняет одну версию centering, symmetric Hann, one-sided нормировки и границ диапазонов. Плагин обязан сравнить `dsp_algorithm_version`, `psd_algorithm_version`, `processing_mode`, окно и шаг входных bundle.

Example находится в `examples/plugins/example-summary/`:

```bash
cargo build --release --manifest-path examples/plugins/example-summary/Cargo.toml
lemon plugins add examples/plugins/example-summary
lemon plugin run example-summary --input ANALYSIS_DIR \
  --config examples/plugins/example-summary/config.json --output OUTPUT_DIR
```

## Другие языки

AnalysisBundle остаётся открытым файловым контрактом, описанным в [analysis.md](analysis.md). Python, Julia, MATLAB и другие среды могут читать `manifest.json`, `summary.json` и CSV напрямую. Они запускаются тем же executable-контрактом и создают тот же `plugin-result.json`; Rust SDK для них не требуется. Entrypoint должен быть заранее подготовленным локальным исполняемым файлом или launcher внутри каталога плагина.

## TUI и ограничения v1

Вкладка Plugins отображает manifest, путь, API, статус, причину и время проверки. Из неё можно обновить статусы, добавить путь, включить, выключить и удалить регистрацию. Она никогда не запускает и не собирает плагины.

Первая версия не включает сеть, marketplace, dynamic libraries, remote API, автоматическую сборку, запуск анализа из TUI или произвольный UI-код. Будущий Visual Plugin API будет декларативным: аналитический плагин вернёт модель таблицы, карточки или графика, а LEMON отрисует её собственными компонентами без исполнения UI-кода внутри процесса LEMON.

## Mu condition analysis

Плагин находится в [`plugins/mu-condition-analysis/`](../plugins/mu-condition-analysis/). Он описательно сравнивает оконную мощность Mu 8–13 Гц и Beta 13–30 Гц между отдельной фоновой записью и отдельными упражнениями.

### 1. Подготовьте background bundle

```bash
lemon analyze "$HOME/Downloads/Лаб2/фон/RECORDING.dmov" \
  --output reports/lab2-background-analysis
```

### 2. Подготовьте bundles упражнений

```bash
lemon analyze "$HOME/Downloads/Лаб2/1 упр - сжатие правой/RECORDING.dmov" \
  --output reports/lab2/right-hand-squeeze-analysis

lemon analyze "$HOME/Downloads/Лаб2/2 упр - воображаемое сжатие/RECORDING.dmov" \
  --output reports/lab2/imagined-squeeze-analysis
```

Проверьте, что каталоги появились:

```bash
ls reports/lab2-background-analysis
ls reports/lab2/right-hand-squeeze-analysis
```

### 3. Соберите плагин

```bash
cargo build --release --locked \
  -p mu-condition-analysis \
  --target-dir plugins/mu-condition-analysis/target
```

Отдельный `--target-dir` нужен потому, что безопасный manifest разрешает entrypoint только внутри каталога плагина.

### 4. Зарегистрируйте плагин

```bash
lemon plugins add plugins/mu-condition-analysis
lemon plugins inspect mu-condition-analysis
```

Ожидаемый статус — `ready`. Если показан `unavailable`, проверьте, что предыдущая сборка завершилась и существует:

```text
plugins/mu-condition-analysis/target/release/mu-condition-analysis
```

### 5. Проверьте конфигурацию

Готовый пример: [`examples/mu-condition-analysis.json`](../examples/mu-condition-analysis.json). Пути condition bundles разрешаются относительно самого JSON-файла.

Основные поля:

```json
{
  "channels": ["A0", "A2"],
  "window_seconds": 2.0,
  "hop_seconds": 0.5,
  "baseline": { "label": "background" }
}
```

### 6. Запустите сравнение

```bash
lemon plugin run mu-condition-analysis \
  --input reports/lab2-background-analysis \
  --config examples/mu-condition-analysis.json \
  --output reports/lab2-mu-condition-results
```

`--input` всегда является фоном. Упражнения перечислены в JSON-конфигурации.

### 7. Посмотрите результат

```bash
cat reports/lab2-mu-condition-results/plugin-result.json
cat reports/lab2-mu-condition-results/summary.json
open reports/lab2-mu-condition-results/figures/mu-band-comparison.png
```

На Linux вместо `open` обычно используется `xdg-open`. Также можно открыть PNG через файловый менеджер.

Основные файлы:

- `channel-condition-results.csv` — медиана, IQR и изменение относительно background;
- `windowed-band-power.csv` — результат каждого валидного окна;
- `excluded-windows.csv` — исключённые интервалы и причины;
- `figures/` — отдельные Mu/Beta и временные графики.

Для v2 плагин принимает только строки `sample_valid=true`, не объединяет разные `segment_id`, читает `excluded-windows.csv` и использует общий PSD из `processing`. Для v1 ограничения пригодности определяются по доступным флагам, значениям и времени с явным предупреждением.

В summary отдельно сохранены версии PSD исходных bundle и `computed_psd_algorithm_version` — версия вычисления новых окон плагином. Там же находятся конфигурации обработки входов и счётчики причин исключения исходного анализа. Эти счётчики не суммируются с исключениями собственной сетки плагина: окна могут отличаться.

Несовместимые частоты, единицы, фильтры, пороги артефактов/качества, прогрев, окно, шаг, диапазоны и версии DSP отклоняются. Различия второстепенных параметров явно перечисляются предупреждением. Mu-плагин пока хранит выбранные Filtered и оконные мощности в памяти; для очень больших наборов потребуется отдельная потоковая доработка.

Это condition comparison между отдельными файлами. Результат не называется MuERD/ERD/ERS: для event-locked анализа нужны метки `baseline`, `cue`, `task` и идентификаторы trial внутри общей временной шкалы. Полная методика описана в [README плагина](../plugins/mu-condition-analysis/README.md).

### Общая сетка окон

Для собственных окон поверх Filtered используйте `processing::windows::{AnalysisSample, WindowingConfig, WindowGenerator}`. Передайте строку через `push`, обработайте `Accepted`/`Rejected` и вызовите `finish` в конце канала. Принятый кандидат содержит значения для `one_sided_psd`; исключённый — типизированные причины с постоянными CSV-кодами.

Mu-плагин использует этот же путь. Для Bundle v2 per-sample metadata является основой пригодности; события агрегированного качества не применяются второй раз. Для v1 адаптер восстанавливает сегменты по sequence/time, учитывает доступные флаги и совпадающие по времени известные события; менее строгая оценка отмечается предупреждением.

Плагин сверяет `windowing_algorithm_version` входных bundle. При его отсутствии вход отмечается старой версией 0; текущая сетка плагина — 1. DSP v2 и v3 нельзя смешивать в одном сравнении: повторно выполните `lemon analyze` для всех условий.
