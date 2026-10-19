# Экспериментальные протоколы

`lemon run experiment` записывает непрерывный EEG-поток и одновременно выполняет заранее проверенный план фаз. Режим сохраняет фактически поставленные маркеры; он не оценивает EEG и не интерпретирует состояние человека.

## Быстрый demo

```bash
lemon run experiment \
  --protocol examples/motor-imagery-demo.json \
  --demo \
  --record
```

Для автоматического теста без TUI разрешено ускорение времени:

```bash
lemon run experiment \
  --protocol examples/motor-imagery-demo.json \
  --demo --headless --time-scale 0.01
```

Experiment mode всегда включает запись, поэтому `--record` можно оставить для явности. `--time-scale` принимается только вместе с `--demo --headless` и не предназначен для исследований.

Live-запуск использует обычную конфигурацию acquisition и DSP:

```bash
lemon run experiment \
  --protocol experiments/motor-imagery-v1.json \
  --config configs/mu-beta.json \
  --port /dev/cu.usbmodem1401 \
  --record
```

Arduino Serial Monitor должен быть закрыт. EEG не останавливается между фазами, при паузе таймера или при переходе к следующему trial.

Для воспроизводимой проверки на native session вместо устройства используйте `--replay recordings/SESSION`; markers получают временную шкалу replay-записи.

## Формат protocol

JSON имеет `schema_version: 1`. Неизвестные поля отклоняются. `id` содержит строчные ASCII-буквы, цифры и дефисы. `preparation_seconds` может быть нулём; длительности baseline, cue, task и rest должны быть конечными и положительными. `condition` — стабильный идентификатор анализа, `label` — инструкция для показа.

`randomization.mode` равен `fixed` или `seeded`. Для `seeded` обязателен `seed`: одинаковые protocol и seed дают одинаковый resolved plan. При `avoid_immediate_repeats: true` LEMON проверяет, что порядок без соседних одинаковых condition возможен, и строит такой порядок детерминированно.

До начала первого trial repetitions разворачиваются в `resolved_plan`. Каждый элемент содержит номер trial, condition, label и четыре фактические длительности. План записывается в `experiment.json` до старта protocol.

## Управление

На экране Experiment:

- `Enter` начинает protocol;
- `Space` ставит на паузу или продолжает только таймер protocol;
- `Esc` открывает подтверждение отмены;
- `Q` использует обычное безопасное завершение LEMON.

Отмена сохраняет Raw, уже записанные события и resolved plan. В headless-режиме protocol начинается автоматически, прогресс печатается в терминал.

## Маркеры и временная шкала

`events.csv` сохраняет `ExperimentStart`, preparation, trial, baseline, cue, task, rest, pause/resume, cancellation и completion markers. Поле `text` содержит компактный JSON с `protocol_id`, `trial_number`, `condition` и `phase`.

Marker timestamp берётся из последней наблюдавшейся временной метки источника и в sidecar обозначается как `last_observed_sample`. Это временная шкала записи, а не wall clock. Для Arduino/Serial без аппаратного trigger нельзя гарантировать субмиллисекундную синхронизацию стимула и EEG; marker не является аппаратным trigger.

## `experiment.json`

Sidecar в каталоге native session обновляется атомарно и содержит:

- версию схемы, ID и SHA-256 исходного protocol;
- randomization mode и seed;
- полный resolved plan;
- первую и последнюю временные метки;
- статус `running`, `completed`, `cancelled` или `failed`;
- причину отмены;
- основу временной шкалы маркеров;
- фактически записанные structured events.

`lemon run analyze` копирует sidecar в AnalysisBundle и добавляет его в `output_files`. Rust-плагины читают его через `lemon_plugin_sdk::Bundle::experiment`; другие языки могут открыть обычный JSON.

## Связь с будущим MuERD

Protocol создаёт event-locked baseline/cue/task/rest markers и повторяемые trials. Следующая отдельная задача сможет использовать их для MuERD. Сам experiment engine не рассчитывает Mu/Beta, не классифицирует сигнал и не делает медицинских, когнитивных или физиологических выводов.
