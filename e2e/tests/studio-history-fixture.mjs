import { expect } from "@playwright/test";
import { getStack } from "./helpers.mjs";

export const FIRST = "11111111-1111-4111-8111-111111111111";
export const SECOND = "22222222-2222-4222-8222-222222222222";
export const STAND = "fixture:stand";
export const ANCHOR = "fixture:anchor";
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

export async function openHistory(page, { placedStand = false, offline = false, beforeMemberReply } = {}) {
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
    zones: [zone(FIRST, "Desk lights", "primary", [anchor, ...(placedStand ? [stand] : [])]),
      zone(SECOND, "Accent lights", "custom", [])],
  };
  let socket;
  let rejectNext = false;
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
      if (path === "/api/v1/scene") return route.fulfill({ json: envelope(scene) });
      if (path === "/api/v1/devices") {
        const items = [device(ANCHOR, "Anchor light"), ...(!offline ? [device(STAND, "Laptop stand")] : [])];
        return route.fulfill({ json: envelope({ items, total: items.length }) });
      }
      // Avoid allowing unrelated persisted layouts to contribute offline cards.
      if (path === "/api/v1/layouts") return route.fulfill({ json: envelope({ items: [], total: 0 }) });
      return route.continue();
    }
    const body = request.postDataJSON();
    const write = { path, method: request.method(), body, revision: request.headers()["if-match"] };
    writes.push(write);
    if (path === "/api/v1/scene/members/edit" && request.method() === "POST") {
      if (beforeMemberReply) await beforeMemberReply(write);
      if (body.scene_id !== scene.id || write.revision !== String(scene.revision) || rejectNext) {
        rejectNext = false;
        rejected.push(write);
        return route.fulfill({ status: 412, json: envelope(scene), headers: { etag: String(scene.revision) } });
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
      scene.revision += 1;
      await route.fulfill({ json: envelope({ document: scene, changes }) });
      notify();
      return;
    }
    const layoutMatch = path.match(/^\/api\/v1\/scene\/zones\/([^/]+)\/layout$/);
    if (layoutMatch && request.method() === "PUT") {
      expect(write.revision).toBe(String(scene.revision));
      const zone = scene.zones.find((zone) => zone.id === layoutMatch[1]);
      zone.layout = body;
      for (const p of body.placements) Object.assign(outputs.get(p.member), p);
      scene.revision += 1;
      await route.fulfill({ json: envelope(zone) });
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

export async function addStand(page) {
  await page.getByTitle("Add to a zone", { exact: true }).click();
  await page.getByRole("button", { name: "Add to Desk lights", exact: true }).click();
}

export async function dragOutput(page, id) {
  const output = page.locator(`[data-zone-id="${id}"]`);
  await output.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const x = Math.round(bounds.x + bounds.width / 2);
    const y = Math.round(bounds.y + bounds.height / 2);
    for (const [type, dx, dy, buttons] of [["mousedown", 0, 0, 1], ["mousemove", 35, 20, 1], ["mouseup", 35, 20, 0]]) {
      element.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, button: 0, buttons, clientX: x + dx, clientY: y + dy }));
    }
  });
}
