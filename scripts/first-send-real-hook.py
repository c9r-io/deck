#!/usr/bin/env python3
"""Record a real Claude UserPromptSubmit event for the opt-in test only.

The hook ignores stdin and stores no prompt, credentials, or transcript text.
Both environment variables must be provided by the disposable test server.
"""

import os
import time

log = os.environ.get("DECK_FIRST_SEND_HOOK_LOG", "")
trial = os.environ.get("DECK_FIRST_SEND_TRIAL", "")
if log.startswith("/tmp/deck-firstsend-") and trial.isascii() and trial.isalnum():
    with open(log, "a", encoding="utf-8") as out:
        out.write(f"{trial} {time.time_ns()}\n")
