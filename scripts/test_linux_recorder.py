"""Linux helper contracts; these tests do not establish native AT-SPI accuracy."""

import importlib.util
from pathlib import Path
import types
import unittest

HELPER = Path(__file__).resolve().parents[1] / "crates/lumo-recorder/src/desktop/platform/linux_capture.py"
SPEC = importlib.util.spec_from_file_location("linux_capture", HELPER)
CAPTURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CAPTURE)
ATSPI = types.SimpleNamespace(
    StateType=types.SimpleNamespace(FOCUSED="focused", EDITABLE="editable"),
    Role=types.SimpleNamespace(PASSWORD_TEXT="password-text"),
)


class Field:
    def __init__(self, focused=True, editable=True, role="text", name="notes", value="合同"):
        self.focused, self.editable = focused, editable
        self.role, self.name, self.value = role, name, value
        self.reads = 0

    def get_state_set(self):
        return types.SimpleNamespace(contains=lambda state: self.focused if state == "focused" else self.editable)

    def get_role_name(self):
        return "文字" if self.role == "password-text" else self.role

    def get_role(self):
        return self.role

    def get_name(self):
        return self.name

    def get_application(self):
        return types.SimpleNamespace(get_name=lambda: "fixture")

    def get_text_iface(self):
        self.reads += 1
        return types.SimpleNamespace(get_character_count=lambda: len(self.value), get_text=lambda *_: self.value)


class AccessibilityTests(unittest.TestCase):
    def setUp(self):
        self.focus = {"safe": True}
        self.events = []
        self.handler = CAPTURE.make_accessibility_handler(ATSPI, self.focus, lambda kind, **payload: self.events.append((kind, payload)))

    def event(self, field, kind="object:text-changed:insert"):
        self.handler(types.SimpleNamespace(type=kind, source=field))

    def test_focus_loss_revokes_previous_field_permission(self):
        self.event(Field(focused=False), "object:state-changed:focused")
        self.assertFalse(self.focus["safe"])
        self.assertEqual(self.events, [])

    def test_failed_accessibility_probe_revokes_permission(self):
        class Vanished(Field):
            def get_state_set(self):
                raise RuntimeError("control disappeared")
        self.event(Vanished(), "object:state-changed:focused")
        self.assertFalse(self.focus["safe"])
        self.assertEqual(self.events, [])

    def test_localized_password_role_never_reads_text(self):
        field = Field(role="password-text", name="", value="secret")
        self.event(field, "object:state-changed:focused")
        self.event(field)
        self.assertFalse(self.focus["safe"])
        self.assertEqual(field.reads, 0)
        self.assertFalse(any(kind == "text" for kind, _ in self.events))

    def test_password_text_event_revokes_stale_safe_focus(self):
        self.event(Field(role="password-text", name="", value="secret"))
        self.assertFalse(self.focus["safe"])
        self.assertEqual(self.events, [])

    def test_only_focused_editable_fields_produce_text(self):
        for field in (Field(focused=False), Field(editable=False), Field(name="密码")):
            self.event(field)
            self.assertEqual(field.reads, 0)
        self.assertEqual(self.events, [])

    def test_safe_unicode_and_empty_deletion_preserve_complete_values(self):
        for value in ("合同🙂", ""):
            self.event(Field(value=value))
        self.assertEqual([payload["text"] for _, payload in self.events], ["合同🙂", ""])
        self.assertTrue(all(payload["safe"] for _, payload in self.events))


class KeyboardTests(unittest.TestCase):
    def test_unsafe_focus_never_serializes_any_key(self):
        for key, modifiers in (("a", ["ctrl"]), ("enter", []), ("f1", []), ("s", ["alt"])):
            self.assertFalse(CAPTURE.capture_key(key, modifiers, False))

    def test_printable_keys_and_shift_only_are_never_serialized(self):
        for key, modifiers in (("a", []), ("a", ["shift"]), ("space", []), ("1", [])):
            self.assertFalse(CAPTURE.capture_key(key, modifiers, True))
        self.assertTrue(CAPTURE.capture_key("s", ["ctrl"], True))
        self.assertTrue(CAPTURE.capture_key("tab", [], True))
        self.assertTrue(CAPTURE.capture_key("f12", [], True))

    def test_modifier_and_unknown_keysyms_never_produce_unreplayable_shortcuts(self):
        for key in ("control_l", "shift_r", "super_l", "multi_key", "é", "f13"):
            self.assertFalse(CAPTURE.capture_key(key, ["ctrl"], True))


if __name__ == "__main__":
    unittest.main()
