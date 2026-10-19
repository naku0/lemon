# Serial и Arduino

Ниже безопасный порядок первого подключения: найти порт, пассивно проверить поток и только затем запустить запись.

## Перед запуском

1. Подключите Arduino.
2. Закройте Arduino Serial Monitor и другие программы, использующие порт.
3. Не открывайте один порт одновременно в двух приложениях.
4. Помните: Arduino UNO может перезапуститься при открытии Serial.

LEMON открывает порт как 8N1 без flow control.

## 1. Найдите порт

```bash
lemon ports
```

Пример на macOS:

```text
/dev/cu.usbmodem1401
```

Имя может измениться после переподключения, поэтому при ошибке `port not found` выполните `lemon ports` снова.

## 2. Проверьте поток без DSP

```bash
lemon probe \
  --port /dev/cu.usbmodem1401 \
  --baud 115200 \
  --duration 10 \
  --output /tmp/lemon-probe.bin \
  --overwrite
```

`probe` только читает байты, ничего не отправляет и не запускает TUI. В конце он показывает размер, скорость, preview и возможную структуру tagged-u8.

## 3. Запустите tagged-u8

Профиль для полного кадра `A0…A7,A9`, выбранных сигналов A0/A1 и счётчика A9 находится в `examples/serial-tagged-u8-a0.json`:

```bash
lemon run live \
  --config examples/serial-tagged-u8-a0.json \
  --port /dev/cu.usbmodem1401 \
  --record
```

Запись появится в `recordings/`. Значения имеют единицы `adc_counts_u8`; LEMON не переводит их в вольты или микровольты без подтверждённой калибровки.

## Один или два выбранных канала

Количество `channels` должно совпадать с длиной `serial_channel_tags`. Текущий example выбирает два канала:

```json
{
  "source": {
    "channels": 2,
    "serial_channel_tags": ["A0", "A1"]
  }
}
```

Для одного A0 создайте копию конфигурации и замените эти поля на:

```json
{
  "source": {
    "channels": 1,
    "serial_channel_tags": ["A0"]
  }
}
```

Полный `serial_frame_tags` остаётся тем же: служебные и невыбранные поля всё равно проверяются для сохранения границ кадра. После редактирования проверьте файл командой `lemon config check FILE`.

## Текстовый CSV-протокол

Для строк `123.5` или `123.5,-42`:

```bash
lemon run live \
  --port PORT \
  --baud 115200 \
  --protocol csv \
  --sample-rate 250 \
  --channels 2 \
  --record
```

Поддерживаются LF, CRLF и строки, разделённые между несколькими Serial-чтениями.

## Как устроен tagged-u8

Serial transport получает произвольные фрагменты байтов. Decoder собирает полный кадр, проверяет порядок тегов и передаёт в LEMON только `serial_channel_tags`:

```text
Serial transport → tagged-u8 decoder → RawSignalBlock → DSP/TUI/storage
```

`serial_sequence_tag` используется для обнаружения потерь и не становится сигнальным каналом. Device timestamp отсутствует, поэтому временная ось восстанавливается по номинальной частоте и отмечается `synchronized_clock=false`.

## Если данные не идут

```bash
lemon ports
lemon probe --port PORT --baud 115200 --duration 10 \
  --output /tmp/check.bin --overwrite
```

Затем проверьте baud, протокол, `serial_frame_tags` и закрыт ли Serial Monitor. Дополнительные решения: [troubleshooting.md](troubleshooting.md#serial-порт-не-найден).
