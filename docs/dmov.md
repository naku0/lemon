# Импорт DMOV

LEMON умеет напрямую читать двухканальные записи Лаб‑2 с тегами A0 и A2. Исходный `.dmov` не изменяется.

## Самый простой импорт

```bash
lemon import \
  "$HOME/Downloads/Лаб2/фон/5_10_2026_12_17_26.dmov" \
  --output recordings/lab2-background
```

После успеха:

```bash
ls recordings/lab2-background
lemon run replay recordings/lab2-background
```

В native-сеансе каналы называются `A0` и `A2`.

## Можно работать без отдельного import

Replay напрямую:

```bash
lemon run replay "$HOME/Downloads/Лаб2/фон/RECORDING.dmov"
```

AnalysisBundle напрямую:

```bash
lemon run analyze "$HOME/Downloads/Лаб2/фон/RECORDING.dmov" \
  --output reports/lab2-background-analysis
```

Notebook напрямую:

```bash
lemon run report create "$HOME/Downloads/Лаб2/фон/RECORDING.dmov" \
  --output reports/lab2-background-report
```

## Как проверить два канала

После `analyze`:

```bash
python3 -c 'import json; print(json.load(open("reports/lab2-background-analysis/manifest.json"))["channels"])'
```

Ожидается:

```text
['A0', 'A2']
```

## Что означает A0 и A2

Проверка с сопутствующими DAT установила техническое соответствие:

```text
A0 → *_EMG.dat
A2 → *_EEG.dat
```

`EEG` и `EMG` здесь являются историческими именами экспортных файлов. Они не доказывают физиологическую модальность. LEMON не назначает A0/A2 позициям C3/C4 и не определяет полушария автоматически.

До калибровки значения сохраняются как `adc_counts_u8`.

## Время и синхронизация

DMOV содержит пакетные ticks, которые сохраняются как события `DmovPacket`. Они не доказывают одновременность отдельных измерений A0 и A2, поэтому:

```text
synchronized_clock=false
```

Временная ось каналов реконструируется без интерполяции и заполнения пропусков нулями. Межканальную фазу, когерентность и асимметрию на таком основании интерпретировать нельзя.

## Что проверяет импортёр

Подтверждённая структура включает длину записи, временное поле и фрагментированные `A<digit><u8>` поля. Импортёр:

- ограничивает длину записи и число полей;
- поддерживает поля, разделённые границами бинарных записей;
- сообщает байтовое смещение повреждения;
- отклоняет оборванный файл;
- сохраняет разрывы как события;
- не принимает неизвестные теги за EEG-каналы.

## Следующий шаг: сравнение условий

Подготовьте отдельный AnalysisBundle для фона и каждого упражнения, затем используйте плагин `mu-condition-analysis`. Пошаговые команды находятся в [plugins.md](plugins.md#mu-condition-analysis).
