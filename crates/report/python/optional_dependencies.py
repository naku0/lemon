# Opt-in only. Standard Google Colab environments normally have these packages.
INSTALL_MISSING_REPORT_PACKAGES = False
import importlib.util
import subprocess
import sys
missing = [name for name in ('numpy', 'pandas', 'matplotlib') if importlib.util.find_spec(name) is None]
if missing:
    if INSTALL_MISSING_REPORT_PACKAGES:
        subprocess.run([sys.executable, '-m', 'pip', 'install', *missing], check=True)
    else:
        print('Missing report packages:', ', '.join(missing))
        print('Install them in your environment, or explicitly set INSTALL_MISSING_REPORT_PACKAGES=True in this cell and rerun it.')
