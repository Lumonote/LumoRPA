"""Expose actionable acceptance failures in GitHub's public job annotations."""

import os


def error_annotation(title, message):
    def escape(value):
        return str(value).replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")
    property_title = escape(title).replace(":", "%3A").replace(",", "%2C")
    return f"::error title={property_title}::{escape(message)}"


def run_acceptance(main, title):
    try:
        return main()
    except Exception as error:
        # Preserve the original failure and traceback locally. CI must expose
        # the actual error rather than only its generic "exit code 1" wrapper.
        if os.environ.get("GITHUB_ACTIONS") == "true":
            print(error_annotation(title, f"{type(error).__name__}: {error}"), flush=True)
        raise
