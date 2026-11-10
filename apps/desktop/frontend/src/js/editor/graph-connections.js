// Data links are source edits, not a second execution graph. A supported link
// reads a preceding sibling's result through the VM's existing template scope.
import { extractSteps, findStepByPath, mutateStepAtPath, parseYaml, walkSteps } from "../yaml.js";

const SAFE_STEP_ID = /^[A-Za-z_][A-Za-z0-9_-]*$/;
const DOT_STEP_ID = /^[A-Za-z_][A-Za-z0-9_]*$/;
const UNSAFE_INPUTS = new Set(["__proto__", "prototype", "constructor"]);

const validPath = (path) => Array.isArray(path) && path.length > 0 &&
  Number.isSafeInteger(path.at(-1)) && path.at(-1) >= 0;
const sameParent = (a, b) => a.length === b.length && a.slice(0, -1).every((part, index) => part === b[index]);
const escapePattern = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Hyphenated IDs require bracket access: the template parser treats '-' as subtraction. */
export function graphResultExpression(stepId) {
  if (typeof stepId !== "string" || !SAFE_STEP_ID.test(stepId)) throw new Error("来源节点 ID 不支持数据连线，请使用字母、数字、下划线或连字符，并以字母或下划线开头");
  const access = DOT_STEP_ID.test(stepId) ? `.${stepId}` : `[${JSON.stringify(stepId)}]`;
  return `{{ steps${access}.result }}`;
}

function isExactResultReference(value, stepId) {
  if (typeof value !== "string") return false;
  const id = escapePattern(stepId);
  const bracket = `\\[\\s*(?:"${id}"|'${id}')\\s*\\]`;
  const access = DOT_STEP_ID.test(stepId) ? `(?:\\.${id}|${bracket})` : bracket;
  return new RegExp(`^\\s*\\{\\{\\s*steps${access}\\.result\\s*\\}\\}\\s*$`).test(value);
}

function readConnection(source, { fromPath, toPath, input }) {
  if (!validPath(fromPath) || !validPath(toPath)) throw new Error("连线节点路径无效，请重新选择端口");
  if (typeof input !== "string" || !input.trim() || UNSAFE_INPUTS.has(input)) throw new Error("目标参数名称无效，无法连接该端口");
  let steps;
  try { steps = extractSteps(parseYaml(source)); }
  catch { throw new Error("流程 YAML 无效，请先在代码视图修复后再连线"); }
  if (!Array.isArray(steps)) throw new Error("流程步骤结构无效，请先修复后再连线");
  const from = findStepByPath(steps, fromPath);
  const to = findStepByPath(steps, toPath);
  if (!from || !to) throw new Error("连线节点已不存在，请重新选择端口");
  if (!sameParent(fromPath, toPath)) throw new Error("数据连线仅支持同一分支内的前序节点，不能跨分支或跨嵌套作用域连接");
  const parent = fromPath.at(-2) === "do" ? findStepByPath(steps, fromPath.slice(0, -2)) : null;
  if (parent?.action === "control.parallel") throw new Error("并行节点的 do 项是独立分支，不能在分支之间连接结果");
  if (fromPath.at(-1) === toPath.at(-1)) throw new Error("节点不能连接自身");
  if (fromPath.at(-1) > toPath.at(-1)) throw new Error("不能引用尚未执行的节点；数据连线必须从前序节点指向后续节点，以避免循环");
  const expression = graphResultExpression(from.id);
  let occurrences = 0;
  walkSteps(steps, (step) => { if (step.id === from.id) occurrences++; });
  if (occurrences !== 1) throw new Error("来源节点 ID 在流程中重复，无法确定结果来源，请先改为唯一 ID");
  if (to.with != null && (typeof to.with !== "object" || Array.isArray(to.with))) throw new Error("目标节点参数不是映射结构，请先在属性或代码视图修复");
  return { from, to, expression };
}

/** Plan an edit only. The caller confirms replacements and snapshots undo before applying it. */
export function planGraphConnection(source, options = {}) {
  const { from, to, expression } = readConnection(source, options);
  const previousValue = to.with?.[options.input];
  const exactReference = isExactResultReference(previousValue, from.id);
  if (options.disconnect && !exactReference) throw new Error("仅可断开完整的结果绑定；混合文本或其他表达式请在属性中编辑");
  if (!options.disconnect && exactReference) return { source, changed: false, previousValue, expression, requiresReplace: false };
  const parameters = { ...(to.with || {}) };
  if (options.disconnect) delete parameters[options.input];
  else parameters[options.input] = expression;
  const nextSource = mutateStepAtPath(source, options.toPath, { ...to, with: parameters });
  const hasValue = previousValue !== undefined && previousValue !== null &&
    (typeof previousValue !== "string" || previousValue.trim() !== "");
  return {
    source: nextSource,
    changed: nextSource !== source,
    previousValue,
    expression,
    requiresReplace: !options.disconnect && hasValue,
  };
}
