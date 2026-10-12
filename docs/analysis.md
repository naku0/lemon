# Технический анализ

```bash
lemon analyze INPUT
```

Результат — самостоятельный AnalysisBundle:

```text
manifest.json
summary.json
summary.md
processed.csv
spectrum.csv
band-power.csv
quality-events.csv
```

`processed.csv` хранит Raw и Filtered отдельно. `spectrum.csv` содержит рассчитанные окна и frequency bins. `band-power.csv` содержит absolute, relative, smoothed и baseline change. `quality-events.csv` хранит инженерные события.

Анализ использует существующий DSP и потоковую запись. Окна FFT не пересчитываются notebook-слоем. Dominant band сравнивается только для валидных непересекающихся диапазонов; одинаковые границы Alpha/Mu группируются. Spectral peak берётся из агрегированного Filtered PSD без DC и в настроенном диапазоне.

Summary формулируется нейтрально: частоты, мощности, число отсчётов, доли quality labels, пропуски и параметры обработки. Он не утверждает, что человек расслаблен, сосредоточен, устал или имеет заболевание.
