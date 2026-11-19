// Bound traversal as well as rendered text: slicing JSON.stringify after the
// fact still serializes a whole large table on the UI thread.
export function previewResult(value, { maxItems = 80, maxDepth = 5, maxString = 1200, maxChars = 12000 } = {}) {
  let remaining = maxItems;
  let truncated = false;
  const seen = new WeakSet();
  const visit = (item, depth) => {
    if (typeof item === "string") {
      if (item.length <= maxString) return item;
      truncated = true; return `${item.slice(0, maxString)}… (${item.length} 字符)`;
    }
    if (!item || typeof item !== "object") return item;
    if (seen.has(item)) { truncated = true; return "[循环引用]"; }
    if (depth >= maxDepth || remaining <= 0) { truncated = true; return Array.isArray(item) ? `[${item.length} 项]` : "[对象]"; }
    seen.add(item);
    const out = Array.isArray(item) ? [] : Object.create(null);
    if (Array.isArray(item)) {
      for (let index = 0; index < item.length; index++) {
        if (remaining-- <= 0) { truncated = true; break; }
        out.push(visit(item[index], depth + 1));
      }
    } else {
      for (const key in item) {
        if (!Object.hasOwn(item, key)) continue;
        if (remaining-- <= 0) { truncated = true; break; }
        out[key] = visit(item[key], depth + 1);
      }
    }
    seen.delete(item);
    return out;
  };
  const preview = visit(value, 0);
  let text = JSON.stringify(preview, null, 2) ?? "（无结果记录）";
  if (text.length > maxChars) { text = `${text.slice(0, maxChars)}\n…`; truncated = true; }
  return { text, truncated };
}
