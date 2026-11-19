// Port types come from action contracts. A dynamic JSON result stays `any`;
// runtime samples never narrow it into a misleading permanent schema.
const isObject = (value) => value && typeof value === "object" && !Array.isArray(value);
const own = (object, key) => isObject(object) && Object.hasOwn(object, key);
const outputSchema = (schema) => own(schema, "x-lumo-output") ? schema["x-lumo-output"] : {};
const valueType = (value) => value === null ? "null" : Array.isArray(value) ? "array" : typeof value === "number" && Number.isInteger(value) ? "integer" : typeof value;

function dereference(schema, root, seen = new Set()) {
  if (!isObject(schema) || !schema.$ref) return schema;
  if (typeof schema.$ref !== "string" || !schema.$ref.startsWith("#/") || seen.has(schema.$ref)) return {};
  const next = new Set([...seen, schema.$ref]);
  let target = root;
  for (const part of schema.$ref.slice(2).split("/")) {
    const key = part.replaceAll("~1", "/").replaceAll("~0", "~");
    if (!own(target, key)) return {};
    target = target[key];
  }
  return dereference(target, root, next);
}

function typesOf(schema, root) {
  schema = dereference(schema, root);
  if (schema === false) return [];
  if (!isObject(schema)) return null;
  let types;
  if (typeof schema.type === "string") types = [schema.type];
  else if (Array.isArray(schema.type)) types = schema.type;
  else if (schema.const !== undefined) types = [valueType(schema.const)];
  else if (Array.isArray(schema.enum) && schema.enum.length) types = schema.enum.map(valueType);
  else if (schema.properties) types = ["object"];
  else if (schema.items) types = ["array"];
  const variants = schema.anyOf || schema.oneOf;
  if (Array.isArray(variants)) {
    const members = variants.map((variant) => typesOf(variant, root));
    if (members.some((member) => member === null)) return types || null;
    const combined = [...new Set(members.flat())];
    types = types ? combined.filter((type) => types.includes(type) || type === "integer" && types.includes("number")) : combined;
  }
  if (schema.nullable && types) types = [...types, "null"];
  return types ? [...new Set(types)].sort() : null;
}

function fieldSchema(schema, key, root) {
  schema = dereference(schema, root);
  if (schema === false) return { exists: false, schema: false };
  if (!isObject(schema)) return { exists: true, schema: {} };
  const variants = schema.anyOf || schema.oneOf;
  if (Array.isArray(variants)) {
    const fields = variants.map((variant) => fieldSchema(variant, key, root)).filter((field) => field.exists);
    return fields.length ? { exists: true, schema: { anyOf: fields.map((field) => field.schema) } } : { exists: false, schema: false };
  }
  const types = typesOf(schema, root);
  if (typeof key === "number") {
    if (types && !types.includes("array")) return { exists: false, schema: false };
    if (Array.isArray(schema.items)) return key < schema.items.length
      ? { exists: true, schema: schema.items[key] } : { exists: schema.additionalItems !== false, schema: schema.additionalItems || {} };
    return { exists: schema.items !== false, schema: schema.items || {} };
  }
  if (types && !types.includes("object")) return { exists: false, schema: false };
  if (own(schema.properties, key)) return { exists: true, schema: schema.properties[key] };
  return { exists: schema.additionalProperties !== false, schema: isObject(schema.additionalProperties) ? schema.additionalProperties : {} };
}

function atPath(schema, path, root) {
  for (const key of path) {
    const field = fieldSchema(schema, key, root);
    if (!field.exists) return field;
    schema = field.schema;
  }
  return { exists: true, schema: dereference(schema, root) };
}

export function graphSchemaType(schema, root = schema) {
  const types = typesOf(schema, root);
  return types?.length ? types.join(" | ") : "any";
}

/** Declared object fields become ports; array indices and arbitrary JSON keys do not. */
export function outputFields(actionSchema) {
  const root = outputSchema(actionSchema);
  const fields = [{ path: [], label: "result", type: graphSchemaType(root), schema: root }];
  const visit = (schema, path, ancestors) => {
    schema = dereference(schema, root);
    if (!isObject(schema) || ancestors.has(schema) || path.length >= 6 || fields.length >= 100) return;
    const next = new Set([...ancestors, schema]);
    for (const [key, child] of Object.entries(isObject(schema.properties) ? schema.properties : {})) {
      if (fields.length >= 100) break;
      const fieldPath = [...path, key];
      fields.push({ path: fieldPath, label: fieldPath.join("."), type: graphSchemaType(child, root), schema: dereference(child, root) });
      visit(child, fieldPath, next);
    }
  };
  visit(root, [], new Set());
  return fields;
}

const accepts = (target, source) => target.includes(source) || source === "integer" && target.includes("number");

function compareSchemas(source, target, sourceRoot, targetRoot, depth = 0) {
  source = dereference(source, sourceRoot); target = dereference(target, targetRoot);
  const from = typesOf(source, sourceRoot), to = typesOf(target, targetRoot);
  if (target === false) return { valid: false, known: true };
  if (!from || !to || depth > 6) return { valid: true, known: false };
  if (!from.every((type) => accepts(to, type))) return { valid: false, known: true };
  if (from.length === 1 && from[0] === "array" && isObject(source) && isObject(target)) {
    if (isObject(source.items) && isObject(target.items)) return compareSchemas(source.items, target.items, sourceRoot, targetRoot, depth + 1);
  }
  if (from.length === 1 && from[0] === "object" && isObject(source) && isObject(target)) {
    let known = true;
    for (const key of target.required || []) {
      const field = fieldSchema(source, key, sourceRoot);
      if (!field.exists) return { valid: false, known: true };
      const comparison = compareSchemas(field.schema, target.properties?.[key] || {}, sourceRoot, targetRoot, depth + 1);
      if (!comparison.valid) return comparison;
      known &&= comparison.known && (source.required || []).includes(key);
    }
    return { valid: true, known };
  }
  return { valid: true, known: true };
}

/** A missing/dynamic contract is connectable with unknown compatibility. */
export function validateGraphTypes(schemaFrom, schemaTo, input, outputPath = []) {
  const sourceRoot = outputSchema(schemaFrom);
  const selected = atPath(sourceRoot, outputPath, sourceRoot);
  const target = fieldSchema(schemaTo || {}, input, schemaTo || {});
  const sourceType = graphSchemaType(selected.schema, sourceRoot);
  const targetType = graphSchemaType(target.schema, schemaTo);
  let result;
  if (!selected.exists) result = { valid: false, known: true, message: "来源输出字段不存在，请重新选择输出端口" };
  else if (!target.exists) result = { valid: false, known: true, message: `目标动作不支持参数 ${input}` };
  else result = compareSchemas(selected.schema, target.schema, sourceRoot, schemaTo || {});
  return { ...result, sourceType, targetType, message: result.message || (!result.valid
    ? `类型不兼容：${sourceType} 输出不能连接到 ${input} (${targetType})，请添加转换节点`
    : result.known ? `${sourceType} → ${targetType}` : "输出或输入类型为动态类型，将在运行时校验") };
}
