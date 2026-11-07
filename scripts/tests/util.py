"""Small file helpers for the script tests that close every file they open."""
import json


def write_json(path, document):
    """Writes `document` as JSON to `path`."""
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(document, handle)


def read_json(path):
    """Reads the JSON document at `path`."""
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def read_text(path):
    """Reads the text file at `path`."""
    with open(path, encoding="utf-8") as handle:
        return handle.read()
