// Line diff for the flow-history preview: trim the common prefix/suffix, run
// an LCS over the middle, and fall back to a block replace when the middle is
// too large for the DP table. Pure module — node --test covered.

export function diffLines(oldText, newText, { maxLcs = 400 } = {}) {
  const a = String(oldText ?? "").split("\n");
  const b = String(newText ?? "").split("\n");
  const same = (text) => ({ type: "same", text });

  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
  }

  const midA = a.slice(start, endA);
  const midB = b.slice(start, endB);
  let middle;
  if (!midA.length && !midB.length) {
    middle = [];
  } else if (midA.length * midB.length > maxLcs * maxLcs) {
    middle = [
      ...midA.map((text) => ({ type: "del", text })),
      ...midB.map((text) => ({ type: "add", text })),
    ];
  } else {
    const m = midA.length;
    const n = midB.length;
    const dp = Array.from({ length: m + 1 }, () => new Uint32Array(n + 1));
    for (let i = m - 1; i >= 0; i--) {
      for (let j = n - 1; j >= 0; j--) {
        dp[i][j] = midA[i] === midB[j]
          ? dp[i + 1][j + 1] + 1
          : Math.max(dp[i + 1][j], dp[i][j + 1]);
      }
    }
    middle = [];
    let i = 0;
    let j = 0;
    while (i < m && j < n) {
      if (midA[i] === midB[j]) {
        middle.push(same(midA[i]));
        i++;
        j++;
      } else if (dp[i + 1][j] >= dp[i][j + 1]) {
        middle.push({ type: "del", text: midA[i++] });
      } else {
        middle.push({ type: "add", text: midB[j++] });
      }
    }
    while (i < m) middle.push({ type: "del", text: midA[i++] });
    while (j < n) middle.push({ type: "add", text: midB[j++] });
  }

  const rows = [...a.slice(0, start).map(same), ...middle, ...a.slice(endA).map(same)];
  const added = rows.filter((r) => r.type === "add").length;
  const removed = rows.filter((r) => r.type === "del").length;
  return { rows, added, removed };
}
