# Архитектура

Поток данных LEMON:

```text
SignalSource → bounded queue → Raw storage → Processor → Broadcast
                                      ├── TUI
                                      ├── analysis
                                      └── optional consumers
```

`source` получает и декодирует данные. `storage` сохраняет Raw до DSP. `processing` выполняет проверку, фильтры, PSD, band power и quality. `analysis` сохраняет численный AnalysisBundle. `report` представляет готовые данные и не повторяет DSP.

Plugin Platform находится после `analysis`:

```text
AnalysisBundle ──→ report
              └─→ plugins registry/validator ──→ отдельный процесс плагина
```

Плагины не входят в live DSP pipeline. Они получают уже созданный неизменяемый bundle, не могут подменять Raw, Filtered или базовые численные результаты и записывают артефакты в отдельный output. `report` и плагины используют публичные файлы bundle; повторять базовый DSP без явно заявленной новой методики не требуется.

Ключевые правила:

- Raw никогда не заменяется Filtered.
- Пропуски не превращаются в нули.
- Очереди ограничены по размеру.
- Медленный необязательный consumer получает счётчик dropped.
- Пауза TUI останавливает отображение, но не запись и обработку.
- DSP-конфигурация не меняется через визуальные настройки активной записи.

Workspace разделён на `core`, `source`, `processing`, `storage`, `analysis`, `report`, `plugins`, `lemon-plugin-sdk`, `tui` и `app`. TUI, report и Plugin Platform не являются зависимостями DSP.
