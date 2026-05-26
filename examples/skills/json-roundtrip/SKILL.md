---
name: json-roundtrip
description: Serialise an object to JSON, re-parse it, return the parsed result.
version: 0.1.0
inputs:
  - { name: payload, type: object, required: true }
outputs:
  - { name: parsed, type: object }
triggers: [json, roundtrip]
---

# json-roundtrip

Used by `skill-driver.yaml` to exercise `skill.invoke` end-to-end with a
non-trivial input that goes through templating + data actions.

```yaml
inputs:
  - { name: payload, type: object, required: true }
outputs:
  - { name: parsed, type: object }
steps:
  - id: dump
    action: data.json_format
    with:
      value: "{{ inputs.payload }}"
      pretty: false
  - id: load
    action: data.json_parse
    with:
      text: "{{ steps.dump.result }}"
  - id: keep
    action: control.set_var
    with:
      name: parsed
      value: "{{ steps.load.result }}"
```
