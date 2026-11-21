#!/usr/bin/env python3
"""Per-test version metadata with delegation to an unchanged protocol fixture."""
import json
import os
from pathlib import Path
import sys
import time

config = json.loads(Path(sys.argv[0]).with_suffix('.json').read_text())
if sys.argv[1:] == ['--version']:
    value = Path(sys.argv[0]).with_suffix('.version').read_text()
    if value == 'fail':
        sys.exit(1)
    if value == 'timeout':
        time.sleep(30)
    print(value)
else:
    os.execv(config['delegate'], [config['delegate']] + sys.argv[1:])
