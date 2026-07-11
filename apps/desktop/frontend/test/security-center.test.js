import test from "node:test";
import assert from "node:assert/strict";

import { renderSecurityCenter, runSecurityAction } from "../src/js/security-center.js";

test("renders grants revocations and L3 biometric boundary without secrets", () => {
  const html = renderSecurityCenter({
    grants: [{ id: "g1", capabilityId: "mcp:mail/send", risk: "L3", scope: "once", arguments: { recipient: "a@b.com", apiKey: "raw-secret" }, expiresAt: "2026-07-12" }],
    findings: [{ id: "f1", kind: "prompt_injection", source: "webpage", summary: "ignored tool expansion" }],
  });
  assert.match(html, /生物认证|BIOMETRIC/);
  assert.match(html, /prompt injection/i);
  assert.doesNotMatch(html, /raw-secret/);
  assert.match(html, /data-security-action="revoke"/);
});

test("maps revoke export and biometric challenge commands", async () => {
  const calls = [];
  for (const action of ["revoke", "export", "biometric"]) await runSecurityAction({ action, grantId: "g1" }, async (cmd, args) => calls.push([cmd, args]));
  assert.deepEqual(calls.map(([cmd]) => cmd), ["security_revoke", "security_export_audit", "security_biometric_challenge"]);
});
