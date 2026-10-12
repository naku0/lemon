# Jupyter-отчёты

`lemon report` создаёт notebook поверх готового AnalysisBundle. Notebook загружает результаты по относительным путям и не реализует повторную фильтрацию, FFT, band power или quality.

```bash
lemon report recording.csv
lemon report recording.csv --no-execute
lemon report recording.csv --portable
lemon report recording.csv --colab
lemon report setup
lemon report doctor
```

Локальный отчёт содержит `report.ipynb`, `report-manifest.json`, `analysis/`, `figures/` и `report-requirements.txt`. `--portable` создаёт ZIP, а `--colab` — notebook с Report Dataset. Абсолютные пути в Python-код не вставляются.

Для выполнения локального notebook LEMON использует отдельное управляемое Python-окружение. Пакеты устанавливаются только явной командой `report setup` или после интерактивного согласия. `--no-execute` не требует Python.
