// The Siontology app's interface. Every call goes to the Rust core through
// Tauri; nothing here talks to the network or holds a key.
"use strict";

const invoke = (command, args) => window.__TAURI__.core.invoke(command, args || {});
const $ = (id) => document.getElementById(id);

const VERIFY_EVERY_MS = 8000;
let lastVerified = null;
let lastVerifiedAt = 0;
let lastPulse = null;

// ---------------------------------------------------------------- helpers
function el(tag, attrs, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs || {})) {
    if (key === "class") node.className = value;
    else if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else node.setAttribute(key, value);
  }
  for (const child of children) {
    if (child === null || child === undefined) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}

let toastTimer = null;
function toast(text, refused) {
  const box = $("toast");
  box.textContent = text;
  box.classList.toggle("refused", Boolean(refused));
  box.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { box.hidden = true; }, refused ? 7000 : 4000);
}

function short(hex) {
  return typeof hex === "string" ? hex.slice(0, 8) : "—";
}

function refusedOf(answer) {
  return answer && answer.status && answer.status !== "Success";
}

// The core proves every receipt of the device's own transactions against
// its Pulse's certified header.
function proven(answer) {
  return Boolean(answer && answer.proof && answer.proof.ok);
}

function proofLine(answer) {
  return answer && answer.proof ? answer.proof.line : "Receipt not proven";
}

function receiptToast(answer) {
  toast(`Pulse ${answer.pulse}: ${answer.receipt}. ${proofLine(answer)}`, refusedOf(answer) || !proven(answer));
}

async function busy(button, work) {
  if (button) {
    button.disabled = true;
    button.setAttribute("aria-busy", "true");
  }
  try {
    return await work();
  } catch (error) {
    toast(String(error), true);
    return null;
  } finally {
    if (button) {
      button.disabled = false;
      button.removeAttribute("aria-busy");
    }
  }
}

function ago(ms) {
  const seconds = Math.max(0, Math.round((Date.now() - ms) / 1000));
  return seconds < 2 ? "just now" : `${seconds} s ago`;
}

// ------------------------------------------------------------------- tabs
function show(view) {
  for (const tab of document.querySelectorAll(".tab")) {
    if (tab.dataset.view === view) tab.setAttribute("aria-current", "page");
    else tab.removeAttribute("aria-current");
  }
  for (const section of document.querySelectorAll(".view")) {
    section.hidden = section.id !== `view-${view}`;
  }
  if (view === "wallet") refreshWallet();
  if (view === "stake") refreshStake();
  if (view === "storage") refreshStorage();
  if (view === "settings") refreshSettings();
}

for (const tab of document.querySelectorAll(".tab")) {
  tab.addEventListener("click", () => show(tab.dataset.view));
}

// ------------------------------------------------------------------- node
function renderVerified(result, history) {
  const card = $("pulse-card");
  const live = $("live");
  const checks = $("checks");
  checks.replaceChildren();
  if (!result) return;
  card.dataset.ok = String(result.ok);
  if (result.ok) {
    $("pulse-label").textContent = "Verified by this device";
    $("pulse-number").textContent = result.pulse === null ? "—" : `Pulse ${Number(result.pulse).toLocaleString("en-US")}`;
    live.dataset.state = "ok";
    $("live-text").textContent = result.pulse === null ? "Verified" : `Pulse ${result.pulse}`;
  } else {
    $("pulse-label").textContent = "Verification failed";
    $("pulse-number").textContent = "Not trusted";
    live.dataset.state = "failed";
    $("live-text").textContent = "Failed";
  }
  for (const raw of result.lines || []) {
    // The core's lines end in "✓" when a check passed; the list marks them.
    const line = raw.replace(/\s*✓$/, "");
    const lower = line.toLowerCase();
    const info = lower.includes("skipped") || lower.includes("not pinned") || lower.includes("on first use")
      || (lower.includes("overdue") && !lower.includes("no overdue"));
    const kind = !result.ok ? "fail" : (info ? "info" : "");
    checks.append(el("li", { class: kind }, line));
  }
  if (history) {
    $("tally-ok").textContent = history.verified;
    $("tally-failed").textContent = history.failed;
  }
}

let verifying = false;

async function verify(button) {
  // One verification at a time: the timer skips a beat rather than pile up.
  if (verifying) return;
  verifying = true;
  try {
    const result = await busy(button, () => invoke("verify"));
    if (!result) return;
    lastVerified = result;
    lastVerifiedAt = Date.now();
    if (result.ok && result.pulse !== null) lastPulse = Number(result.pulse);
    refreshStakeLive();
    const history = await invoke("history").catch(() => null);
    renderVerified(result, history);
    const settings = await invoke("settings").catch(() => null);
    if (settings) $("tally-chain").textContent = short(settings.chain);
    if (result.pinned_now) {
      toast(`Pinned chain ${short(settings && settings.chain)} on first use. Compare it with one from a source you trust, in Settings.`);
    }
  } finally {
    verifying = false;
  }
}

$("verify-now").addEventListener("click", (event) => verify(event.currentTarget));

setInterval(() => {
  if (lastVerifiedAt) {
    // Only a skipped certificate means unchecked signatures; a state proof
    // skipped because the node moved on says nothing about them.
    const skipped = lastVerified && (lastVerified.lines || []).some((line) => line.toLowerCase().includes("simulation signatures"));
    const checked = skipped ? "receipts and data checked; signatures not checked (simulation signatures)" : "signatures, receipts and data checked";
    $("pulse-when").textContent = `${ago(lastVerifiedAt)} · ${lastVerified && lastVerified.ok ? checked : "the node's answer did not check out"}`;
  }
}, 1000);
setInterval(() => verify(null), VERIFY_EVERY_MS);

// ----------------------------------------------------------------- wallet
let account = null;

async function refreshWallet() {
  const me = await invoke("me").catch(() => null);
  if (me) {
    account = me.account;
    $("account").textContent = short(me.account);
    $("account").title = me.account;
  }
  const balance = await invoke("balance").catch((error) => { toast(String(error), true); return null; });
  if (balance) $("balance").textContent = balance.usd;
}

$("copy-account").addEventListener("click", async () => {
  if (!account) return;
  try {
    await navigator.clipboard.writeText(account);
    toast("Account copied");
  } catch (_) {
    toast(account);
  }
});

$("send-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = event.submitter;
  const answer = await busy(button, () => invoke("send", { to: $("send-to").value.trim(), amount: $("send-amount").value.trim() }));
  if (!answer) return;
  const receipt = $("send-receipt");
  receipt.hidden = false;
  receipt.replaceChildren(
    el("span", {}, `Pulse ${answer.pulse}: ${answer.receipt}`),
    el("span", { class: "proof" }, proofLine(answer)));
  receipt.classList.toggle("refused", refusedOf(answer) || !proven(answer));
  if (!refusedOf(answer)) $("send-form").reset();
  refreshWallet();
});

$("faucet").addEventListener("click", async (event) => {
  const answer = await busy(event.currentTarget, () => invoke("faucet", { amount: "100" }));
  if (answer) {
    toast("Requested 100 test USD; it arrives within a few Pulses");
    setTimeout(refreshWallet, 3000);
  }
});

// ------------------------------------------------------------------ stake
let acting = 0;

async function refreshStake() {
  const [pools, mine] = await Promise.all([
    invoke("pools").catch((error) => { toast(String(error), true); return null; }),
    invoke("delegations").catch(() => null),
  ]);
  if (pools) renderPools(pools);
  if (mine) renderMine(mine);
}

// Pools and delegations change with the epoch (pending, earning,
// withdrawable): while the tab is open they follow each verification,
// except while one of its actions is under way.
async function refreshStakeLive() {
  if ($("view-stake").hidden || acting > 0) return;
  const [pools, mine] = await Promise.all([
    invoke("pools").catch(() => null),
    invoke("delegations").catch(() => null),
  ]);
  if (acting > 0) return;
  if (pools) renderPools(pools);
  if (mine) renderMine(mine);
}

// One row per validator, built once and then updated in place, so that
// an amount being typed survives a refresh.
const validatorRows = new Map();

function validatorRow(v) {
  const name = el("strong", {});
  const weight = el("span", { class: "weight" });
  const own = el("b", {});
  const pooled = el("b", {});
  const commission = el("b", {});
  const amount = el("input", { inputmode: "decimal", placeholder: "amount, at least 1", "aria-label": `Amount to delegate to ${v.short}` });
  const form = el("form", {
    onsubmit: async (event) => {
      event.preventDefault();
      acting += 1;
      const answer = await busy(event.submitter, () => invoke("delegate", { validator: v.validator, amount: amount.value.trim() }));
      acting -= 1;
      if (!answer) return;
      receiptToast(answer);
      if (!refusedOf(answer)) amount.value = "";
      refreshStake();
    },
  }, amount, el("button", { class: "button button--small", type: "submit" }, "Delegate"));
  const node = el("div", { class: "row" },
    el("div", { class: "row-head" }, name, el("span", { class: "mono" }, v.short), weight),
    el("div", { class: "facts" },
      el("span", {}, "own ", own),
      el("span", {}, "pooled ", pooled),
      el("span", {}, "commission ", commission)),
    form);
  const update = (v) => {
    name.textContent = v.operator;
    weight.textContent = v.weight === null ? v.status.toLowerCase() : `weight ${Number(v.weight).toLocaleString("en-US")}`;
    own.textContent = `${v.own} test USD`;
    pooled.textContent = `${v.delegated} test USD`;
    commission.textContent = v.next_commission && v.next_commission !== v.commission
      ? `${v.commission}, ${v.next_commission} from the epoch's end` : v.commission;
  };
  return { node, update };
}

function renderPools(pools) {
  const length = pools.epoch_length > 1e12 ? "an epoch that never ends" : `${pools.epoch_length}-Pulse epochs`;
  $("epoch-note").textContent = `Epoch ${pools.epoch}, ${length}. A delegation counts from the end of the epoch it is made in.`;
  const rows = $("validators");
  const seen = new Set();
  for (const v of pools.validators) {
    seen.add(v.validator);
    let row = validatorRows.get(v.validator);
    if (!row) {
      row = validatorRow(v);
      validatorRows.set(v.validator, row);
      rows.append(row.node);
    }
    row.update(v);
  }
  for (const [id, row] of validatorRows) {
    if (!seen.has(id)) {
      row.node.remove();
      validatorRows.delete(id);
    }
  }
}

function renderMine(mine) {
  const box = $("mine");
  box.replaceChildren();
  if (!mine.delegations.length) {
    box.append(el("p", { class: "empty" }, "None yet. Pick a validator below."));
    return;
  }
  for (const d of mine.delegations) {
    const act = (action, label, disabled) => {
      const button = el("button", {
        class: "button button--small button--quiet",
        onclick: async (event) => {
          acting += 1;
          const answer = await busy(event.currentTarget, () => invoke("delegation_action", { action, delegation: d.delegation }));
          acting -= 1;
          if (!answer) return;
          receiptToast(answer);
          refreshStake();
          refreshWallet();
        },
      }, label);
      button.disabled = Boolean(disabled);
      return button;
    };
    // Only what the protocol allows in each phase (specification §5.46–5.48).
    const owed = d.rewards_owed !== null && Number(d.rewards_owed) > 0;
    const actions = [];
    if (d.phase === "earning") actions.push(act("claim", "Collect", !owed));
    if (d.phase === "pending" || d.phase === "earning") actions.push(act("undelegate", "Leave"));
    if (d.phase === "unbonding") actions.push(act("withdraw", "Withdraw", lastPulse !== null && d.withdrawable_at > lastPulse));
    box.append(el("div", { class: "card" },
      el("div", { class: "row-head" }, el("strong", {}, `${d.stake} test USD`), el("span", { class: "mono" }, `with ${short(d.validator)}`)),
      el("div", { class: "state" }, phaseLabel(d), d.rewards_owed !== null ? ` · ${d.rewards_owed} test USD to collect` : ""),
      el("div", { class: "actions" }, ...actions)));
  }
}

// How a delegation's phase reads (the protocol's names are for the API).
function phaseLabel(d) {
  if (d.phase === "pending") return "Pending: counts from the end of this epoch";
  if (d.phase === "earning") return "Active";
  if (d.phase === "unbonding") return `Leaving: withdrawable from Pulse ${d.withdrawable_at}`;
  return d.state;
}

// ---------------------------------------------------------------- storage
let storageEnabled = false;
let storageTicking = false;
const storageLog = [];

function renderStorage(status) {
  storageEnabled = Boolean(status.enabled);
  $("storage-toggle").textContent = storageEnabled ? "Stop providing" : "Provide storage";
  $("storage-toggle").classList.toggle("button--quiet", storageEnabled);
  if (document.activeElement !== $("storage-mib")) $("storage-mib").value = status.max_mib;
  $("st-kept").textContent = status.kept;
  $("st-collected").textContent = status.collected;
  $("st-due").textContent = status.due;
  const mib = (Number(status.bytes) / (1024 * 1024)).toFixed(1);
  $("st-note").textContent = storageEnabled
    ? `Providing: ${mib} MiB kept, ${status.at_stake} test USD of collateral at stake.`
    : `Off. ${mib} MiB kept for deals already taken.`;
  const rows = $("st-deals");
  rows.replaceChildren();
  if (!status.deals.length) {
    rows.append(el("p", { class: "empty" }, storageEnabled ? "Waiting for an open deal this device can fetch." : "None yet."));
  }
  for (const d of status.deals) {
    const state = d.phase === "closed"
      ? `closed: ${d.proven} of ${d.rounds} rounds proven${d.closed && Number(d.closed.forfeited) > 0 ? `, ${d.closed.forfeited} test USD forfeited` : ""}`
      : `${d.proven} of ${d.rounds} rounds proven${d.round_now !== null ? `, round ${d.round_now} under way` : ""}`;
    rows.append(el("div", { class: "row" },
      el("div", { class: "row-head" },
        el("strong", {}, `${d.leaves} leaves`), el("span", { class: "mono" }, d.short),
        el("span", { class: "weight" }, `${d.price} test USD a round`)),
      el("div", { class: "facts" }, el("span", {}, state))));
  }
}

async function refreshStorage() {
  const status = await invoke("storage_status").catch((error) => { toast(String(error), true); return null; });
  if (status) renderStorage(status);
}

// One provider step every two seconds, so that a round's proof goes out
// early in the round. The core takes new deals only while providing, and
// keeps proving the ones it holds either way.
async function storageTick() {
  if (storageTicking) return;
  storageTicking = true;
  try {
    const result = await invoke("storage_tick").catch((error) => ({ lines: [`Error: ${error}`] }));
    if (!(result.lines || []).length) return;
    for (const line of result.lines) {
      storageLog.unshift(line);
    }
    storageLog.splice(20);
    const log = $("st-log");
    log.replaceChildren(...storageLog.map((line) => el("li", { class: line.startsWith("Error") || line.includes("Failed") ? "fail" : "" }, line)));
    if (!$("view-storage").hidden) refreshStorage();
  } finally {
    storageTicking = false;
  }
}

setInterval(storageTick, 2000);

$("storage-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const maxMib = Number($("storage-mib").value);
  const answer = await busy(event.submitter, () => invoke("set_storage", { enabled: !storageEnabled, maxMib }));
  if (answer) {
    toast(answer.provide_storage ? "Providing storage: open deals this device can fetch will be taken" : "No new deals; the ones kept are still proven until they end, while the app runs");
    storageEnabled = Boolean(answer.provide_storage);
    refreshStorage();
    storageTick();
  }
});

// --------------------------------------------------------------- settings
async function refreshSettings() {
  const settings = await invoke("settings").catch(() => null);
  if (!settings) return;
  $("node-url").value = settings.node;
  $("pinned").textContent = settings.chain
    || (settings.pin_on_first_use
      ? "Not pinned yet: the next verification pins what the node serves."
      : "Not pinned, and nothing will be pinned on first use: pin an identity you trust.");
  if (settings.notice) toast(settings.notice, true);
}

$("node-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const answer = await busy(event.submitter, () => invoke("set_node", { node: $("node-url").value.trim() }));
  if (answer) {
    toast(`Node set to ${answer.node}`);
    verify(null);
  }
});

$("pin-form").addEventListener("submit", async (event) => {
  event.preventDefault();
  const answer = await busy(event.submitter, () => invoke("pin_chain", { chain: $("pin-input").value.trim() }));
  if (answer) {
    toast(`Pinned ${short(answer.chain)}`);
    $("pin-input").value = "";
    refreshSettings();
    verify(null);
  }
});

$("unpin").addEventListener("click", async (event) => {
  const answer = await busy(event.currentTarget, () => invoke("pin_chain", { chain: null }));
  if (answer) {
    toast("Unpinned: the next verification pins what the node serves");
    refreshSettings();
  }
});

// ------------------------------------------------------------------ start
refreshWallet();
refreshStorage();
verify(null);
