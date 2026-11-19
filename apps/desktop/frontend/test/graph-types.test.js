import assert from "node:assert/strict";
import test from "node:test";
import { graphSchemaType, outputFields, validateGraphTypes } from "../src/js/editor/graph-types.js";

const output = (schema) => ({ "x-lumo-output": schema });
const input = (schema) => ({ type: "object", properties: { value: schema }, additionalProperties: false });

test("output ports expose only declared result fields and retain arbitrary JSON as any", () => {
  assert.deepEqual(outputFields({}), [{ path: [], label: "result", type: "any", schema: {} }]);
  const schema = output({ type: "object", properties: {
    status: { type: "integer" }, json: {},
    usage: { type: "object", properties: { tokens: { type: "integer" } } },
    rows: { type: "array", items: { type: "object", properties: { name: { type: "string" } } } },
  } });
  assert.deepEqual(outputFields(schema).map(({ path, label, type }) => ({ path, label, type })), [
    { path: [], label: "result", type: "object" },
    { path: ["status"], label: "status", type: "integer" },
    { path: ["json"], label: "json", type: "any" },
    { path: ["usage"], label: "usage", type: "object" },
    { path: ["usage", "tokens"], label: "usage.tokens", type: "integer" },
    { path: ["rows"], label: "rows", type: "array" },
  ]);
});

test("type checks permit integer widening and dynamic JSON without inventing compatibility", () => {
  assert.deepEqual(validateGraphTypes(output({ type: "integer" }), input({ type: "number" }), "value"), {
    valid: true, known: true, sourceType: "integer", targetType: "number", message: "integer → number",
  });
  for (const source of [{}, { properties: { json: {} }, type: "object" }]) {
    const path = source.properties ? ["json", "arbitrary"] : [];
    const result = validateGraphTypes(output(source), input({ type: "string" }), "value", path);
    assert.equal(result.valid, true);
    assert.equal(result.known, false);
    assert.equal(result.sourceType, "any");
  }
  assert.equal(validateGraphTypes(output({ type: "number" }), input({ type: "integer" }), "value").valid, false);
  assert.equal(validateGraphTypes(output({ type: "boolean" }), input({ type: "string" }), "value").valid, false);
});

test("union and array contracts reject known incompatible alternatives", () => {
  assert.equal(graphSchemaType({ anyOf: [{ type: "string" }, { type: "null" }] }), "null | string");
  assert.equal(validateGraphTypes(output({ type: ["string", "null"] }), input({ type: "string" }), "value").valid, false);
  assert.equal(validateGraphTypes(output({ type: "integer" }), input({ anyOf: [{ type: "string" }, { type: "number" }] }), "value").valid, true);
  assert.equal(validateGraphTypes(output({ type: "array", items: { type: "string" } }), input({ type: "array", items: { type: "integer" } }), "value").valid, false);
  const item = validateGraphTypes(output({ type: "array", items: { type: "string" } }), input({ type: "string" }), "value", [0]);
  assert.equal(item.sourceType, "string");
  assert.equal(item.valid, true);
});

test("closed contracts reject missing ports and optional source keys remain uncertain", () => {
  const closed = output({ type: "object", properties: { value: { type: "string" } }, additionalProperties: false });
  assert.match(validateGraphTypes(closed, input({ type: "string" }), "value", ["absent"]).message, /不存在/);
  assert.equal(validateGraphTypes(closed, input({ type: "string" }), "absent").valid, false);
  const target = input({ type: "object", required: ["value"], properties: { value: { type: "string" } } });
  assert.equal(validateGraphTypes(closed, target, "value").known, false);
  assert.equal(validateGraphTypes(output({ ...closed["x-lumo-output"], required: ["value"] }), target, "value").known, true);
  assert.equal(validateGraphTypes(output({ type: "object", additionalProperties: false }), target, "value").valid, false);
});

test("local references resolve with bounded recursion and do not resolve remote contracts", () => {
  const schema = output({ $ref: "#/$defs/response", $defs: { response: { type: "object", properties: { text: { type: "string" } } } } });
  assert.equal(outputFields(schema)[1].type, "string");
  assert.equal(validateGraphTypes(schema, input({ type: "string" }), "value", ["text"]).valid, true);
  assert.equal(outputFields(output({ $ref: "#/cycle", cycle: { $ref: "#/cycle" } }))[0].type, "any");
  assert.equal(outputFields(output({ $ref: "https://example.com/schema" }))[0].type, "any");
});
