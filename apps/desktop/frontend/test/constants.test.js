import assert from "node:assert/strict";
import test from "node:test";

import { categoryOf, zhAction } from "../src/js/constants.js";

test("categoryOf groups implemented but previously hidden action families", () => {
  assert.equal(categoryOf("docx.read_text"), "docx");
  assert.equal(categoryOf("xml.xpath"), "xml");
  assert.equal(categoryOf("window.activate"), "desktop");
});

test("zhAction gives user-facing labels for advanced implemented actions", () => {
  const ids = [
    "browser.wait_response",
    "browser.extract_table",
    "http.paginate",
    "http.oauth2_token",
    "file.append",
    "file.wait",
    "excel.write_range",
    "excel.set_style",
    "excel.lookup",
    "email.move",
    "docx.read_text",
    "string.encode_convert",
    "xml.xpath",
    "db.sqlite_batch",
    "db.postgres_query",
    "db.mysql_exec",
    "system.process_kill",
    "desktop.click_text",
    "window.activate",
  ];

  for (const id of ids) {
    assert.notEqual(zhAction(id).label, id, `${id} should not fall back to raw id`);
    assert.ok(zhAction(id).hint.length > 0, `${id} should have a short hint`);
  }
});
