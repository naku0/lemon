# Частые проблемы

## Команда `lemon` не найдена

Проверьте установленный бинарник:

```bash
ls "$HOME/.cargo/bin/lemon"
```

Если файла нет:

```bash
cargo install --path crates/app --locked --force
```

Если файл есть, добавьте Cargo bin в `PATH` для zsh:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
which lemon
```

## Запускается старая версия

`cargo build` обновляет `./target/debug/lemon`, а команда `lemon` обычно указывает на установленный бинарник.

```bash
which lemon
./target/debug/lemon --version
lemon --version
```

Обновить установленную команду:

```bash
cargo install --path crates/app --locked --force
```

## Cargo не может создать временный каталог на macOS

Характерная ошибка:

```text
couldn't create a temp dir: Permission denied ... /var/folders/.../T/
```

Проверьте временный каталог:

```bash
echo "$TMPDIR"
python3 -c 'import tempfile; print(tempfile.gettempdir())'
```

Для текущего терминала можно вернуть стандартный выбор системы:

```bash
unset TMPDIR
cargo build --locked
```

Если системный каталог по-прежнему недоступен, временный обход для одной команды:

```bash
mkdir -p /tmp/lemon-cargo-tmp
TMPDIR=/tmp/lemon-cargo-tmp cargo build --locked
```

Это не настройка LEMON и не должно требоваться при исправных правах системного temp.

## Serial-порт не найден

Сначала найдите актуальное имя:

```bash
lemon ports
```

На macOS имя может измениться после переподключения, например `/dev/cu.usbmodem1401`. Закройте Arduino Serial Monitor: одновременно порт обычно может использовать только одна программа.

Проверьте поток пассивным чтением:

```bash
lemon probe --port PORT --baud 115200 \
  --duration 10 --output /tmp/lemon-probe.bin --overwrite
```

`probe` ничего не отправляет устройству.

## Notebook не выполняется

Проверьте управляемое окружение:

```bash
lemon report doctor
```

Если оно отсутствует или повреждено:

```bash
lemon report setup --recreate
lemon report doctor
```

LEMON создаёт отдельный venv в каталоге данных приложения. Пакеты не нужно устанавливать в системный Python.

Если notebook открыт вручную в VS Code или Jupyter, выберите kernel из пути, который показывает `lemon report doctor`. Иначе редактор может использовать `/usr/local/bin/python3` без `matplotlib` и `ipykernel`.

## Output уже существует

LEMON защищает существующие результаты. Выберите новый путь:

```bash
lemon analyze INPUT --output reports/analysis-2
```

Либо явно разрешите замену точно указанного каталога:

```bash
lemon analyze INPUT --output reports/analysis --overwrite
```

Проверяйте путь перед `--overwrite`.

## Плагин имеет статус `unavailable`

Посмотрите причину:

```bash
lemon plugins inspect PLUGIN_ID
```

Для плагинов на Rust обычно сначала нужен release-бинарник внутри каталога плагина. Для Mu-плагина:

```bash
cargo build --release --locked \
  -p mu-condition-analysis \
  --target-dir plugins/mu-condition-analysis/target

lemon plugins add plugins/mu-condition-analysis
```

Регистрация не собирает плагин автоматически.

## Нужна более подробная ошибка

```bash
lemon --log-level debug COMMAND
```

Для конкретной команды также используйте справку:

```bash
lemon COMMAND --help
```
