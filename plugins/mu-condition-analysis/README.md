# Mu-band condition comparison

## Быстрый запуск

Если AnalysisBundle для фона и упражнений уже созданы:

```bash
# Собрать плагин внутрь его каталога
cargo build --release --locked \
  -p mu-condition-analysis \
  --target-dir plugins/mu-condition-analysis/target

# Подключить один раз
lemon plugins add plugins/mu-condition-analysis
lemon plugins inspect mu-condition-analysis

# Запустить
lemon plugin run mu-condition-analysis \
  --input reports/lab2-background-analysis \
  --config examples/mu-condition-analysis.json \
  --output reports/lab2-mu-condition-results
```

Посмотреть созданные файлы:

```bash
cat reports/lab2-mu-condition-results/plugin-result.json
ls reports/lab2-mu-condition-results/figures
```

Полный путь от DMOV до результата разобран в [общем руководстве по плагинам](../../docs/plugins.md#mu-condition-analysis).

## Назначение

Плагин описательно сравнивает мощность отфильтрованного сигнала в диапазонах Mu 8–13 Гц и Beta 13–30 Гц между отдельной фоновой записью и отдельными записями условий. Он сохраняет результаты каждого окна и не формулирует физиологических, когнитивных, клинических или диагностических выводов.

## Ограничения метода

Это **condition comparison**, а не event-locked MuERD/ERD/ERS. В отдельных файлах нет общих меток `baseline_start`, `cue`, `task_start`, `task_end`. Плагин не рассчитывает p-value и не делает выводов о статистической значимости. Отрицательное число означает только `decrease relative to background`, положительное — `increase relative to background`.

Имена A0/A2 сохраняются без назначения C3/C4, полушария или контралатеральности. При `synchronized_clock=false` нельзя интерпретировать межканальную фазу, когерентность или асимметрию. Имена `_EEG.dat` и `_EMG.dat` не используются как доказательство разных физиологических модальностей.

Окна с пропусками, разрывами, flags и привязанными ко времени событиями плохого качества исключаются. Если сам AnalysisBundle предупреждает, что sample-level quality получена из односэмпловых блоков, плагин не принимает такие `Poor` labels за достоверную оценку целого окна. Это решение явно записывается в `summary.json` и warnings; остальные признаки разрыва и качества продолжают применяться.

## Подготовка AnalysisBundle

Сначала импортируйте/проанализируйте фон и каждое условие одинаковой конфигурацией DSP:

```bash
lemon analyze "$HOME/Downloads/Лаб2/фон/RECORDING.dmov" \
  --output reports/lab2-background-analysis
lemon analyze "$HOME/Downloads/Лаб2/1 упр - сжатие правой/RECORDING.dmov" \
  --output reports/lab2/right-hand-squeeze-analysis
```

Реальные записи и созданные bundles не добавляются в Git.

## Сборка и подключение

Entrypoint обязан находиться внутри каталога плагина. Соберите его в локальный `target` плагина:

```bash
cargo build --release -p mu-condition-analysis \
  --target-dir plugins/mu-condition-analysis/target
lemon plugins add plugins/mu-condition-analysis
```

## Запуск

```bash
lemon plugin run mu-condition-analysis \
  --input reports/lab2-background-analysis \
  --config examples/mu-condition-analysis.json \
  --output reports/lab2-mu-condition-results
```

`--input` всегда задаёт background bundle. Пути условий разрешаются относительно файла конфигурации. `mode` равен `condition-comparison`; `channels`, границы диапазонов, окно, шаг, baseline label и conditions задаются явно. Все bundles должны содержать выбранные каналы и иметь одинаковые sample rate, units и `processing_config`.

## Артефакты

- `summary.json` — метод, версии входов, параметры, числа окон, предупреждения и ограничения;
- `channel-condition-results.csv` — медиана, IQR и изменения относительно background;
- `windowed-band-power.csv` — мощность Mu/Beta каждого валидного окна;
- `excluded-windows.csv` — интервал и причина исключения;
- `figures/mu-band-comparison.png` и `beta-band-comparison.png`;
- `figures/mu-power-over-time-CHANNEL.png` для каждого канала с валидными окнами;
- `plugin-result.json` — проверяемый контракт Plugin Platform.

Пустые графики не создаются. Если данных недостаточно, таблицы и summary сохраняются, а `plugin-result.json` получает `completed_with_warnings`.

## Как читать результат

`relative_change_percent = 100 × (P_condition / P_background − 1)`. `change_db = 10 × log10(P_condition / P_background)`. Это описательное отношение медиан окон мощности. Оно само по себе не устанавливает причину изменения и не подтверждает реакцию участника.

## Будущий event-locked MuERD

Для следующего режима нужны синхронизированные временные метки событий как минимум `baseline_start`, `baseline_end`, `cue`, `task_start`, `task_end`, идентификатор trial и правила отклонения trial. Только после этого можно задавать event-relative окна, нормализацию baseline и агрегирование повторов под названием MuERD/ERS.
