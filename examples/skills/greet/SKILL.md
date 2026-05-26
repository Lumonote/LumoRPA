---
name: greet
description: Reusable greeter — emits one log line and exposes the message as `greeting`.
version: 0.1.0
inputs:
  - { name: who, type: string, required: true, default: world }
outputs:
  - { name: greeting, type: string }
triggers: [greet, hello]
tags: [demo, hello-world]
author: lumorpa
---

# greet

A trivial skill used to smoke-test the LumoRPA skill subsystem.

```yaml
inputs:
  - { name: who, type: string, required: true, default: world }
outputs:
  - { name: greeting, type: string }
steps:
  - id: say
    action: control.log
    with:
      message: "hello, {{ inputs.who }}!"
  - id: store
    action: control.set_var
    with:
      name: greeting
      value: "hello, {{ inputs.who }}!"
```
