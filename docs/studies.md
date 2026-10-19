# Offline study

`lemon run study` подготавливает несколько отдельных записей одинаковым DSP-процессом.
Одна запись явно назначается фоном, остальные — условиями. Команда подходит для
Лаб‑2, где фон и упражнения сохранены отдельными CSV, DAT или DMOV.

Она не запускает Mu-плагин и не интерпретирует сигнал. Результат — набор
`AnalysisBundle` и готовая конфигурация для `mu-condition-analysis`.

## Быстрый запуск

Подготовьте каталог:

```text
study/
  study.json
  background/recording.dmov
  right-hand-squeeze/recording.dmov
  imagined-right-hand-squeeze/recording.dmov
```

Пути в `study.json` задаются относительно самого manifest:

```json
{
  "schema_version": 1,
  "id": "lab2-mu-offline",
  "display_name": "Lab 2 Mu offline comparison",
  "description": "Separate recordings compared with a background recording.",
  "baseline": {
    "label": "background",
    "input": "background/recording.dmov"
  },
  "conditions": [
    {
      "label": "right-hand-squeeze",
      "input": "right-hand-squeeze/recording.dmov"
    },
    {
      "label": "imagined-right-hand-squeeze",
      "input": "imagined-right-hand-squeeze/recording.dmov"
    }
  ]
}
```

Запустите подготовку:

```bash
lemon run study \
  --manifest study/study.json \
  --output reports/lab2-study \
  --config examples/mu-beta.json
```

Если отдельный DSP-config не нужен, удалите `--config`: применяется обычный порядок
настроек LEMON. Чтобы заменить существующий study целиком, добавьте `--overwrite`.

## Результат

```text
reports/lab2-study/
  study-manifest.json
  background/
    manifest.json
    summary.json
    processed.csv
    ...
  right-hand-squeeze/
    ...
  imagined-right-hand-squeeze/
    ...
  mu-condition-analysis.json
```

Каждый вложенный каталог — самостоятельный `AnalysisBundle`. Все записи проходят
одни настройки обработки. `study-manifest.json` хранит SHA-256 DSP-конфигурации,
SHA-256 источников, относительные пути bundle, предупреждения импорта и версию LEMON.

Публикация атомарна: пока обрабатываются источники, результат находится во временном
каталоге рядом с output. Если хотя бы один источник повреждён, итоговый output не
появится, а существующий output при `--overwrite` останется прежним.

## Запуск Mu condition analysis

Сначала соберите и зарегистрируйте плагин, если это ещё не сделано:

```bash
cargo build --release -p mu-condition-analysis
lemon plugins add plugins/mu-condition-analysis
```

Затем используйте фон как основной input и сгенерированную конфигурацию:

```bash
lemon run plugin mu-condition-analysis \
  --input reports/lab2-study/background \
  --config reports/lab2-study/mu-condition-analysis.json \
  --output reports/lab2-study-mu-results
```

Пути condition bundle внутри конфигурации относительны каталогу study, поэтому весь
`reports/lab2-study` можно переместить целиком.

## Проверки manifest

- `schema_version` первой версии равен `1`;
- `id` содержит строчные ASCII-буквы, цифры и дефисы;
- baseline обязателен, condition требуется хотя бы один;
- labels непустые, уникальные и используются как имена каталогов результата;
- input должен быть относительным путём к существующему файлу или каталогу;
- один канонический источник нельзя назначить двум ролям;
- неизвестные JSON-поля отклоняются;
- роль записи определяется только manifest, а не именем файла.

CSV, DAT и DMOV проходят существующий import/analyze pipeline. Для DMOV действуют те
же companion rules; исторические `_EEG.dat` и `_EMG.dat` не считаются доказательством
физиологической модальности.

## Study и experiment

| Режим | Вход | Следующий анализ |
|---|---|---|
| `lemon run study` | Отдельные фоновые и condition-записи | `mu-condition-analysis` |
| `lemon run experiment` | Одна непрерывная запись с trial markers | `mu-erd` |

Offline comparison не знает точного момента начала упражнения внутри отдельного
файла. Поэтому он сравнивает распределения мощности по записям и не является
event-locked анализом. Для временной привязки к `TaskStart` нужна новая запись через
experiment mode.

Study допускает один общий канал. Он не назначает A0/A2 позициям C3/C4, не строит
межканальные выводы при `synchronized_clock=false` и не формулирует выводы о состоянии
или намерении человека.
