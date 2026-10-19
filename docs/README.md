# Документация LEMON

Рабочие режимы запускаются через `lemon run …`. Реестр плагинов управляется
через `lemon plugins …`, настройки — через `lemon config …`; `import`, `export`,
`ports` и `probe` остаются отдельными утилитами.

Выберите страницу по задаче:

## Начало работы

- [Быстрый старт](getting-started.md) — собрать LEMON, записать demo, создать анализ и notebook;
- [Частые проблемы](troubleshooting.md) — PATH, старая версия, TMPDIR, Serial, Python и плагины;
- [Settings и темы](settings.md) — управление интерфейсом.

## Источники данных

- [Serial и Arduino](serial.md) — порт, безопасный `probe`, CSV и tagged-u8;
- [DMOV](dmov.md) — прямой импорт двух каналов Лаб‑2.

## Результаты

- [AnalysisBundle](analysis.md) — JSON/CSV от `lemon run analyze`;
- [Offline study](studies.md) — пакетная подготовка отдельных фоновых и condition-записей;
- [Jupyter-отчёты](reports.md) — local, portable ZIP и Colab;
- [Плагины](plugins.md) — подключение, запуск и Mu condition analysis.
- [Эксперименты](experiments.md) — trial plan, фазовые markers и запись sidecar.

## Для разработчиков

- [Архитектура](architecture.md);
- [Разработка и проверки](development.md);
- [Наблюдения по исходным BiTronics-файлам](bitronics-observations.md);
- [Архив прежней полной справки](reference.md).

Каждая пользовательская страница начинается с готовых команд. Детали форматов и ограничения находятся ниже на той же странице.
