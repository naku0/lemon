# Serial и Arduino

Serial разделён на два уровня:

```text
Serial transport → SerialDecoder → RawSignalBlock → DSP/TUI/storage
```

Transport открывает 8N1 без flow control и получает произвольные фрагменты байтов. Decoder знает структуру протокола, но не открывает порт.

Поддерживаются:

- `csv`: строки `123.5` или `123.5,-42`, LF/CRLF, ограниченная длина строки;
- `tagged-u8`: пары `A<цифра>` + бинарный байт.

Для `tagged-u8` `serial_frame_tags` описывает полный транспортный кадр, `serial_channel_tags` выбирает 1–2 сигнала, а `serial_sequence_tag` является служебным счётчиком modulo 256. Остальные поля проверяются, но не передаются в DSP.

```bash
lemon live --port /dev/cu.usbmodem1401 --baud 115200 \
  --protocol tagged-u8 --sample-rate 333.333 --channels 1 --record
```

`probe` ничего не отправляет устройству:

```bash
lemon probe --port /dev/cu.usbmodem1401 --baud 115200 \
  --duration 10 --output capture.bin
```

UNO может перезапуститься при открытии порта, поэтому предусмотрена стартовая задержка. USB-время прихода не используется как точная временная метка. Для Arduino `map(analogRead(...), 0, 1023, 0, 255)` единицы — `adc_counts_u8`; физическая калибровка отсутствует.
