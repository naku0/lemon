# Архитектура

Поток данных LEMON:

```text
SignalSource → bounded queue → Raw storage → Processor → Broadcast
                                      ├── TUI
                                      ├── analysis
                                      └── optional consumers
```

`source` получает и декодирует данные. `storage` сохраняет Raw до DSP. `processing` выполняет проверку, фильтры, PSD, band power и quality. `analysis` сохраняет численный AnalysisBundle. `report` представляет готовые данные и не повторяет DSP.

Ключевые правила:

- Raw никогда не заменяется Filtered.
- Пропуски не превращаются в нули.
- Очереди ограничены по размеру.
- Медленный необязательный consumer получает счётчик dropped.
- Пауза TUI останавливает отображение, но не запись и обработку.
- DSP-конфигурация не меняется через визуальные настройки активной записи.

Workspace разделён на `core`, `source`, `processing`, `storage`, `analysis`, `report`, `tui` и `app`. TUI и report не являются зависимостями DSP.
