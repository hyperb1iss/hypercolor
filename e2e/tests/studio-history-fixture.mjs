import { randomUUID } from "node:crypto";

import { expect } from "@playwright/test";
import { daemonError, daemonHeaders, getStack, parseFence, refuseFence, staleFence, writeFence } from "./helpers.mjs";

export const FIRST = "11111111-1111-4111-8111-111111111111";
export const SECOND = "22222222-2222-4222-8222-222222222222";
export const STAND = "fixture:stand";
export const ANCHOR = "fixture:anchor";
export const BASE = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
export const RIPPLES = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
export const HAZE = "cccccccc-cccc-4ccc-8ccc-cccccccccccc";
const layer = (id, name, scope) => ({
  id, name, source: { type: "color_fill", rgba: [0.2, 0.4, 0.6, 1] },
  blend: "alpha", opacity: 1,
  transform: { anchor: { x: 0.5, y: 0.5 }, scale: [1, 1], rotation: 0, fit: "cover" },
  adjust: { brightness: 1, saturation: 1, hue_shift: 0, tint: [1, 1, 1, 1], tint_strength: 0, contrast: 0 },
  enabled: true,
  ...(scope ? { scope: { outputs: scope } } : {}),
});
const sameScope = (a, b) => JSON.stringify([...(a?.outputs ?? [])].sort()) === JSON.stringify([...(b?.outputs ?? [])].sort());
const output = (id, device, name) => ({
  id, device_id: device, name, zone_name: null,
  position: { x: 0.35, y: 0.4 }, size: { x: 0.18, y: 0.1 },
  rotation: 0, scale: 1, display_order: 0,
  topology: { type: "strip", count: 12, direction: "left_to_right" },
  orientation: null, sampling_mode: null, edge_behavior: null,
  shape: null, shape_preset: null,
});
const member = (output) => ({ id: output.id, device_id: output.device_id, name: output.name, segment: output.zone_name });
const placement = (output) => ({
  member: output.id, position: output.position, size: output.size,
  rotation: output.rotation, scale: output.scale, orientation: output.orientation,
  topology: output.topology,
});
const device = (id, name) => ({
  id, layout_device_id: id, name,
  origin: { driver_id: "fixture", backend_id: "usb", transport: "usb", protocol_id: "fixture/test" },
  presentation: { label: "Fixture" }, status: "connected", brightness: 100, total_leds: 12, segments: [],
});

// `standLayer` stacks the first zone as Base, Ripples limited to the stand,
// and Haze over both, and the stand-in then moves layers as a daemon does:
// by the `layer_moves` a write names, or, when it names none, by carrying a
// layer whose outputs all left to the top of the stack they went to.
export async function openHistory(page, { placedStand = false, offline = false, standLayer = false, beforeMemberReply } = {}) {
  const anchor = output("anchor-output", ANCHOR, "Anchor light");
  const stand = output("stand-output", STAND, "Laptop stand");
  const outputs = new Map([[anchor.id, anchor], ...(placedStand ? [[stand.id, stand]] : [])]);
  const zone = (id, name, role, members) => ({
    id, name, role, enabled: true, brightness: 1,
    members: members.map(member), layout: { placements: members.map(placement) }, layers: [],
  });
  const scene = {
    id: "33333333-3333-4333-8333-333333333333", name: "History fixture",
    kind: "named", is_default: false, revision: 42,
    run_epoch: "0b6f2b57-6e0c-4d7c-9a51-7c1d1f0e2a10",
    zones: [zone(FIRST, "Desk lights", "primary", [anchor, ...(placedStand ? [stand] : [])]),
      zone(SECOND, "Accent lights", "custom", [])],
  };
  if (standLayer) {
    scene.zones[0].layers = [layer(BASE, "Base"), layer(RIPPLES, "Ripples", [stand.id]), layer(HAZE, "Haze")];
  }
  const zoneOf = (id) => scene.zones.find((zone) => zone.id === id);
  // The layer moves a membership write commits, or the refusal a daemon
  // answers moves that do not fit the stacks as they stand.
  const planLayerMoves = (body, changes) => {
    if (body.layer_moves) {
      for (const move of body.layer_moves) {
        const [from, to] = [zoneOf(move.from.zone_id), zoneOf(move.to.zone_id)];
        const found = from?.layers[move.from.index];
        if (!to || !found || found.id !== move.layer_id || !sameScope(found.scope, move.scope_before)) {
          return { refusal: daemonError(scene, 409, "conflict",
            `layer ${move.layer_id} is no longer where the membership receipt left it`) };
        }
        const arriving = body.layer_moves.filter((other) => other.to.zone_id === to.id).length;
        const leaving = body.layer_moves.filter((other) => other.from.zone_id === to.id).length;
        const length = to.layers.length - leaving + arriving;
        if (move.to.index >= length) {
          return { refusal: daemonError(scene, 422, "validation_error",
            `layer move index ${move.to.index} is out of range for a stack of ${length}, or names a slot twice`) };
        }
      }
      return { moves: structuredClone(body.layer_moves) };
    }
    const moves = [];
    const landed = new Map();
    for (const change of changes) {
      if (!change.before || !change.after || change.before.zone_id === change.after.zone_id) continue;
      const [from, to] = [zoneOf(change.before.zone_id), zoneOf(change.after.zone_id)];
      from.layers.forEach((stored, index) => {
        if (!stored.scope || !stored.scope.outputs.every((id) => id === change.before.output.id)) return;
        const carried = landed.get(to.id) ?? 0;
        landed.set(to.id, carried + 1);
        moves.push({
          layer_id: stored.id, from: { zone_id: from.id, index }, to: { zone_id: to.id, index: to.layers.length + carried },
          scope_before: structuredClone(stored.scope), scope_after: structuredClone(stored.scope),
        });
      });
    }
    return { moves };
  };
  const applyLayerMoves = (moves) => {
    const lifted = moves.map((move) => {
      const from = zoneOf(move.from.zone_id);
      const [stored] = from.layers.splice(from.layers.findIndex((candidate) => candidate.id === move.layer_id), 1);
      return { move, stored };
    });
    for (const { move, stored } of lifted.sort((a, b) => a.move.to.index - b.move.to.index)) {
      stored.scope = structuredClone(move.scope_after);
      zoneOf(move.to.zone_id).layers.splice(move.to.index, 0, stored);
    }
  };
  let socket;
  let rejectNext = false;
  let refuseNext = null;
  const writes = [];
  const rejected = [];
  const unexpected = [];
  const envelope = (data) => ({ data, meta: { api_version: "v1", request_id: "history_fixture", timestamp: "2026-09-10T00:00:00Z" } });
  const notify = () => socket.send(JSON.stringify({ type: "event", event: "zone_changed", data: {
    scene_id: scene.id, zone_id: FIRST, role: "primary", kind: "updated",
  } }));
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (route) => { socket = route; });
  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (["GET", "HEAD"].includes(request.method())) {
      if (path === "/api/v1/scene") {
        return route.fulfill({ json: envelope(scene), headers: daemonHeaders(scene, { fence: true }) });
      }
      if (path === "/api/v1/devices") {
        const items = [device(ANCHOR, "Anchor light"), ...(!offline ? [device(STAND, "Laptop stand")] : [])];
        return route.fulfill({ json: envelope({ items, total: items.length }), headers: daemonHeaders(scene) });
      }
      // Avoid allowing unrelated persisted layouts to contribute offline cards.
      if (path === "/api/v1/layouts") {
        return route.fulfill({ json: envelope({ items: [], total: 0 }), headers: daemonHeaders(scene) });
      }
      if (path === "/api/v1/layouts/active") {
        return route.fulfill({ json: envelope({
          id: "fixture-rig", name: "Fixture rig", description: null,
          canvas_width: 640, canvas_height: 480, zones: [], version: 1,
        }), headers: daemonHeaders(scene) });
      }
      return route.continue();
    }
    const body = request.postDataJSON();
    const ifMatch = request.headers()["if-match"];
    const write = { path, method: request.method(), body, ...writeFence(ifMatch) };
    writes.push(write);
    if (path === "/api/v1/scene/members/edit" && request.method() === "POST") {
      if (beforeMemberReply) await beforeMemberReply(write);
      // In the daemon's order: the fence, then the scene it names. A forced
      // rejection stands in for a commit elsewhere that the fence missed.
      const refusal = refuseFence(scene, ifMatch, { required: true })
        ?? (rejectNext ? staleFence(scene, parseFence(ifMatch)) : null)
        ?? (body.scene_id !== scene.id ? daemonError(scene, 409, "conflict", "the active scene changed") : null);
      if (refusal) {
        rejectNext = false;
        rejected.push(write);
        return route.fulfill(refusal);
      }
      if (refuseNext) {
        const answer = daemonError(scene, ...refuseNext);
        refuseNext = null;
        rejected.push(write);
        return route.fulfill(answer);
      }
      let changes = structuredClone(body.changes);
      if (body.assignment) {
        expect(changes).toEqual([]);
        const assignment = body.assignment;
        expect(assignment.device_id).toBe(STAND);
        expect(assignment.segments).toEqual([]);
        expect(assignment.placements ?? []).toEqual([]);
        const target = scene.zones.find((zone) => zone.id === assignment.zone_id);
        expect(target).toBeDefined();
        const owner = scene.zones.find((zone) => zone.members.some((member) => member.device_id === STAND));
        const existing = owner?.members.find((member) => member.device_id === STAND);
        const canonical = existing ? outputs.get(existing.id) : stand;
        changes = [{
          before: existing ? { zone_id: owner.id, output: canonical, index: owner.members.indexOf(existing) } : null,
          after: { zone_id: target.id, output: structuredClone(canonical), index: target.members.length },
        }];
      }
      // Validate every precondition before mutating any membership.
      for (const change of changes) {
        if (!change.before) continue;
        const source = scene.zones.find((zone) => zone.id === change.before.zone_id);
        expect(source.members[change.before.index]?.id).toBe(change.before.output.id);
        change.before.output = structuredClone(outputs.get(change.before.output.id));
      }
      const layers = planLayerMoves(body, changes);
      if (layers.refusal) {
        rejected.push(write);
        return route.fulfill(layers.refusal);
      }
      for (const change of changes) {
        if (!change.before) continue;
        const source = scene.zones.find((zone) => zone.id === change.before.zone_id);
        source.members = source.members.filter((member) => member.id !== change.before.output.id);
        source.layout.placements = source.layout.placements.filter((placement) => placement.member !== change.before.output.id);
        outputs.delete(change.before.output.id);
      }
      for (const change of changes) {
        if (!change.after) continue;
        const target = scene.zones.find((zone) => zone.id === change.after.zone_id);
        const next = structuredClone(change.after.output);
        outputs.set(next.id, next);
        target.members.splice(change.after.index, 0, member(next));
        target.layout.placements.splice(change.after.index, 0, placement(next));
      }
      applyLayerMoves(layers.moves);
      scene.revision += 1;
      await route.fulfill({
        json: envelope({ document: scene, changes, layer_moves: layers.moves }),
        headers: daemonHeaders(scene, { fence: true }),
      });
      notify();
      return;
    }
    if (path === "/api/v1/scene/zones" && request.method() === "POST") {
      expect(refuseFence(scene, ifMatch), `zone create fenced at ${ifMatch}`).toBeNull();
      const made = zone(randomUUID(), body.name, "custom", []);
      scene.zones.push(made);
      scene.revision += 1;
      await route.fulfill({ status: 201, json: envelope(made), headers: daemonHeaders(scene, { fence: true }) });
      notify();
      return;
    }
    const zoneMatch = path.match(/^\/api\/v1\/scene\/zones\/([^/]+)$/);
    if (zoneMatch && request.method() === "DELETE") {
      expect(refuseFence(scene, ifMatch), `zone delete fenced at ${ifMatch}`).toBeNull();
      scene.zones = scene.zones.filter((stored) => stored.id !== zoneMatch[1]);
      scene.revision += 1;
      await route.fulfill({ json: envelope(scene), headers: daemonHeaders(scene, { fence: true }) });
      notify();
      return;
    }
    const layoutMatch = path.match(/^\/api\/v1\/scene\/zones\/([^/]+)\/layout$/);
    if (layoutMatch && request.method() === "PUT") {
      // Layout writes in these specs are always current, run and revision.
      expect(refuseFence(scene, ifMatch), `layout write fenced at ${ifMatch}`).toBeNull();
      const zone = scene.zones.find((zone) => zone.id === layoutMatch[1]);
      zone.layout = body;
      for (const p of body.placements) Object.assign(outputs.get(p.member), p);
      scene.revision += 1;
      await route.fulfill({ json: envelope(zone), headers: daemonHeaders(scene, { fence: true }) });
      notify();
      return;
    }
    unexpected.push(write);
    await route.abort("blockedbyclient");
  });
  await page.goto(getStack().appOrigin + "/studio", { waitUntil: "networkidle" });
  await expect.poll(() => Boolean(socket)).toBe(true);
  await expect(page.locator('[data-zone-id="anchor-output"]')).toBeVisible();
  return {
    scene, writes, rejected, unexpected,
    rejectNext: () => { rejectNext = true; },
    // The next membership write is refused as the daemon refuses a request
    // it read and will not apply: `refuseNext(422, "validation_error", "...")`.
    refuseNext: (...refusal) => { refuseNext = refusal; },
    stack: (zoneId) => zoneOf(zoneId)?.layers.map((stored) => stored.id),
    remotePlacement: (id, fields) => {
      Object.assign(outputs.get(id), fields);
      for (const zone of scene.zones) {
        const existing = zone.layout.placements.find((placement) => placement.member === id);
        if (existing) Object.assign(existing, fields);
      }
      scene.revision += 1;
      notify();
    },
    assigned: (deviceId = STAND) => scene.zones.flatMap((zone) => zone.members
      .filter((member) => member.device_id === deviceId).map((member) => ({ zone: zone.id, id: member.id }))),
    undo: page.getByRole("button", { name: "Undo (Ctrl+Z)", exact: true }),
    redo: page.getByRole("button", { name: "Redo (Ctrl+Shift+Z)", exact: true }),
  };
}

export async function addStand(page, zone = "Desk lights") {
  await page.getByTitle(`Add to ${zone}`, { exact: true }).click();
}

export async function dragOutput(page, id) {
  const output = page.locator(`[data-zone-id="${id}"]`);
  await output.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const x = Math.round(bounds.x + bounds.width / 2);
    const y = Math.round(bounds.y + bounds.height / 2);
    for (const [type, dx, dy, buttons] of [["pointerdown", 0, 0, 1], ["pointermove", 35, 20, 1], ["pointerup", 35, 20, 0]]) {
      element.dispatchEvent(new PointerEvent(type, {
        bubbles: true, cancelable: true, pointerId: 1, pointerType: "mouse", isPrimary: true,
        button: 0, buttons, clientX: x + dx, clientY: y + dy,
      }));
    }
  });
}
