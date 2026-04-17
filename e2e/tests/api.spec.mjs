import { test, expect } from "@playwright/test";

import {
  buildAttachmentTemplate,
  createApi,
  findRunnableEffect,
  firstControlPayload,
  readEnvelope,
  readJson,
  uniqueName,
} from "./helpers.mjs";

test.describe("REST API", () => {
  test("system endpoints expose live daemon state", async ({ playwright }) => {
    const api = await createApi(playwright);

    try {
      const healthResponse = await api.get("/health");
      expect(healthResponse.ok()).toBeTruthy();
      const health = await readJson(healthResponse);
      expect(health.status).toBe("healthy");

      const status = await readEnvelope(await api.get("/api/v1/status"));
      expect(status.running).toBe(true);
      expect(status.version).toBeTruthy();

      expect((await api.get("/api/v1/server")).ok()).toBeTruthy();
      expect((await api.post("/api/v1/diagnose")).ok()).toBeTruthy();
      expect((await api.get("/api/v1/system/sensors")).ok()).toBeTruthy();
    } finally {
      await api.dispose();
    }
  });

  test("effects endpoints can apply, update, reset, rescan, and stop", async ({
    playwright,
  }) => {
    const api = await createApi(playwright);

    try {
      expect((await api.post("/api/v1/effects/rescan")).ok()).toBeTruthy();

      const effects = await readEnvelope(await api.get("/api/v1/effects"));
      const runnableEffect = findRunnableEffect(effects.items, ["Audio Pulse", "Gradient", "Rainbow"]);
      expect(runnableEffect.runnable).toBe(true);

      const detail = await readEnvelope(await api.get(`/api/v1/effects/${runnableEffect.id}`));
      expect(detail.id).toBe(runnableEffect.id);

      await readEnvelope(await api.post(`/api/v1/effects/${runnableEffect.id}/apply`));
      const active = await readEnvelope(await api.get("/api/v1/effects/active"));
      expect(active.id).toBe(runnableEffect.id);

      const controls = firstControlPayload(active);
      expect(
        (
          await api.patch("/api/v1/effects/current/controls", {
            data: {
              controls,
            },
          })
        ).ok(),
      ).toBeTruthy();

      expect((await api.post("/api/v1/effects/current/reset")).ok()).toBeTruthy();
      expect((await api.post("/api/v1/effects/stop")).ok()).toBeTruthy();

      const status = await readEnvelope(await api.get("/api/v1/status"));
      expect(status.active_effect).toBeNull();
    } finally {
      await api.dispose();
    }
  });

  test("layouts and scenes round-trip through create, update, apply, and delete", async ({
    playwright,
  }) => {
    const api = await createApi(playwright);
    const layoutName = uniqueName("e2e-layout");
    const updatedLayoutName = `${layoutName}-updated`;
    const sceneName = uniqueName("e2e-scene");
    const updatedSceneName = `${sceneName}-updated`;

    let layoutId = null;
    let sceneId = null;

    try {
      const createdLayout = await readEnvelope(
        await api.post("/api/v1/layouts", {
          data: {
            name: layoutName,
            canvas_width: 320,
            canvas_height: 200,
          },
        }),
      );
      layoutId = createdLayout.id;
      expect(createdLayout.name).toBe(layoutName);

      const layoutList = await readEnvelope(await api.get("/api/v1/layouts"));
      expect(layoutList.items.some((item) => item.id === layoutId)).toBe(true);

      const fetchedLayout = await readEnvelope(await api.get(`/api/v1/layouts/${layoutId}`));
      expect(fetchedLayout.id).toBe(layoutId);

      const updatedLayout = await readEnvelope(
        await api.put(`/api/v1/layouts/${layoutId}`, {
          data: {
            name: updatedLayoutName,
            canvas_width: 400,
            canvas_height: 240,
          },
        }),
      );
      expect(updatedLayout.name).toBe(updatedLayoutName);

      expect((await api.post(`/api/v1/layouts/${layoutId}/apply`)).ok()).toBeTruthy();
      expect(
        (
          await api.put("/api/v1/layouts/active/preview", {
            data: fetchedLayout,
          })
        ).ok(),
      ).toBeTruthy();

      const createdScene = await readEnvelope(
        await api.post("/api/v1/scenes", {
          data: {
            name: sceneName,
            mutation_mode: "live",
          },
        }),
      );
      sceneId = createdScene.id;
      expect(createdScene.name).toBe(sceneName);

      const scenes = await readEnvelope(await api.get("/api/v1/scenes"));
      expect(scenes.items.some((scene) => scene.id === sceneId)).toBe(true);

      expect((await api.post(`/api/v1/scenes/${sceneId}/activate`)).ok()).toBeTruthy();

      const activeScene = await readEnvelope(await api.get("/api/v1/scenes/active"));
      expect(activeScene.id).toBe(sceneId);

      const updatedScene = await readEnvelope(
        await api.put(`/api/v1/scenes/${sceneId}`, {
          data: {
            name: updatedSceneName,
            mutation_mode: "snapshot",
          },
        }),
      );
      expect(updatedScene.name).toBe(updatedSceneName);

      expect((await api.post("/api/v1/scenes/deactivate")).ok()).toBeTruthy();
    } finally {
      if (sceneId) {
        await api.delete(`/api/v1/scenes/${sceneId}`);
      }
      if (layoutId) {
        await api.delete(`/api/v1/layouts/${layoutId}`);
      }
      await api.dispose();
    }
  });

  test("library and settings endpoints round-trip state", async ({ playwright }) => {
    const api = await createApi(playwright);
    const presetName = uniqueName("e2e-preset");
    const updatedPresetName = `${presetName}-updated`;
    const playlistName = uniqueName("e2e-playlist");
    const updatedPlaylistName = `${playlistName}-updated`;

    let presetId = null;
    let playlistId = null;

    try {
      const effects = await readEnvelope(await api.get("/api/v1/effects"));
      const audioPulse = findRunnableEffect(effects.items, ["Audio Pulse", "Gradient", "Rainbow"]);

      const brightnessBefore = await readEnvelope(await api.get("/api/v1/settings/brightness"));
      const restoredBrightness = brightnessBefore.brightness;

      const brightnessAfter = await readEnvelope(
        await api.put("/api/v1/settings/brightness", {
          data: {
            brightness: 37,
          },
        }),
      );
      expect(brightnessAfter.brightness).toBe(37);

      expect((await api.get("/api/v1/config")).ok()).toBeTruthy();
      expect((await api.get("/api/v1/audio/devices")).ok()).toBeTruthy();

      expect(
        (
          await api.post("/api/v1/config/set", {
            data: {
              key: "network.mdns_publish",
              value: "false",
              live: false,
            },
          })
        ).ok(),
      ).toBeTruthy();
      expect(
        (
          await api.post("/api/v1/config/reset", {
            data: {
              key: "network.mdns_publish",
              live: false,
            },
          })
        ).ok(),
      ).toBeTruthy();

      expect(
        (
          await api.post("/api/v1/library/favorites", {
            data: {
              effect: audioPulse.id,
            },
          })
        ).ok(),
      ).toBeTruthy();
      const favorites = await readEnvelope(await api.get("/api/v1/library/favorites"));
      expect(favorites.items.some((entry) => entry.effect_id === audioPulse.id)).toBe(true);
      expect((await api.delete(`/api/v1/library/favorites/${audioPulse.id}`)).ok()).toBeTruthy();

      const preset = await readEnvelope(
        await api.post("/api/v1/library/presets", {
          data: {
            name: presetName,
            effect: audioPulse.id,
            controls: {},
          },
        }),
      );
      presetId = preset.id;
      expect(preset.name).toBe(presetName);

      expect((await api.post(`/api/v1/library/presets/${presetId}/apply`)).ok()).toBeTruthy();

      const updatedPreset = await readEnvelope(
        await api.put(`/api/v1/library/presets/${presetId}`, {
          data: {
            name: updatedPresetName,
            effect: audioPulse.id,
            controls: {},
          },
        }),
      );
      expect(updatedPreset.name).toBe(updatedPresetName);

      const playlist = await readEnvelope(
        await api.post("/api/v1/library/playlists", {
          data: {
            name: playlistName,
            loop_enabled: false,
            items: [
              {
                target: {
                  type: "effect",
                  effect: audioPulse.id,
                },
                duration_ms: 500,
                transition_ms: 0,
              },
            ],
          },
        }),
      );
      playlistId = playlist.id;

      expect((await api.post(`/api/v1/library/playlists/${playlistId}/activate`)).ok()).toBeTruthy();
      const activePlaylist = await readEnvelope(
        await api.get("/api/v1/library/playlists/active"),
      );
      expect(activePlaylist.playlist.id).toBe(playlistId);

      const updatedPlaylist = await readEnvelope(
        await api.put(`/api/v1/library/playlists/${playlistId}`, {
          data: {
            name: updatedPlaylistName,
            loop_enabled: false,
            items: [
              {
                target: {
                  type: "effect",
                  effect: audioPulse.id,
                },
                duration_ms: 750,
                transition_ms: 0,
              },
            ],
          },
        }),
      );
      expect(updatedPlaylist.name).toBe(updatedPlaylistName);

      expect((await api.post(`/api/v1/library/playlists/${playlistId}/activate`)).ok()).toBeTruthy();
      expect((await api.post("/api/v1/library/playlists/stop")).ok()).toBeTruthy();

      await readEnvelope(
        await api.put("/api/v1/settings/brightness", {
          data: {
            brightness: restoredBrightness,
          },
        }),
      );
    } finally {
      if (playlistId) {
        await api.delete(`/api/v1/library/playlists/${playlistId}`);
      }
      if (presetId) {
        await api.delete(`/api/v1/library/presets/${presetId}`);
      }
      await api.post("/api/v1/library/playlists/stop");
      await api.post("/api/v1/effects/stop");
      await api.dispose();
    }
  });

  test("simulators, displays, devices, and attachment templates work end to end", async ({
    playwright,
  }) => {
    const api = await createApi(playwright);
    const simulatorName = uniqueName("e2e-simulator");
    const updatedSimulatorName = `${simulatorName}-updated`;
    const templateId = uniqueName("e2e-template");

    let simulatorId = null;

    try {
      const discoveryResponse = await api.post("/api/v1/devices/discover", {
        data: {
          backends: ["blocks"],
          wait: false,
        },
      });
      expect([202, 409]).toContain(discoveryResponse.status());
      if (discoveryResponse.status() === 202) {
        const discovery = await readEnvelope(discoveryResponse);
        expect(discovery.status).toBe("scanning");
      }

      const simulator = await readEnvelope(
        await api.post("/api/v1/simulators/displays", {
          data: {
            name: simulatorName,
            width: 320,
            height: 320,
            circular: true,
            enabled: true,
          },
        }),
      );
      simulatorId = simulator.id;

      const simulators = await readEnvelope(await api.get("/api/v1/simulators/displays"));
      expect(simulators.some((entry) => entry.id === simulatorId)).toBe(true);

      const updatedSimulator = await readEnvelope(
        await api.patch(`/api/v1/simulators/displays/${simulatorId}`, {
          data: {
            name: updatedSimulatorName,
            width: 360,
            height: 360,
            circular: false,
          },
        }),
      );
      expect(updatedSimulator.name).toBe(updatedSimulatorName);

      const displays = await readEnvelope(await api.get("/api/v1/displays"));
      expect(displays.some((display) => display.id === simulatorId)).toBe(true);

      const devices = await readEnvelope(await api.get("/api/v1/devices"));
      expect(devices.items.some((device) => device.id === simulatorId)).toBe(true);
      expect((await api.get(`/api/v1/devices/${simulatorId}`)).ok()).toBeTruthy();

      expect((await api.get(`/api/v1/devices/${simulatorId}/attachments`)).ok()).toBeTruthy();
      const categories = await readEnvelope(await api.get("/api/v1/attachments/categories"));
      expect(categories.items.length).toBeGreaterThan(0);
      const vendors = await readEnvelope(await api.get("/api/v1/attachments/vendors"));
      expect(vendors.items.length).toBeGreaterThan(0);

      const createdTemplate = await readEnvelope(
        await api.post("/api/v1/attachments/templates", {
          data: buildAttachmentTemplate(templateId, "E2E Template", 12),
        }),
      );
      expect(createdTemplate.id).toBe(templateId);

      const templateDetail = await readEnvelope(
        await api.get(`/api/v1/attachments/templates/${templateId}`),
      );
      expect(templateDetail.id).toBe(templateId);

      const updatedTemplate = await readEnvelope(
        await api.put(`/api/v1/attachments/templates/${templateId}`, {
          data: buildAttachmentTemplate(templateId, "E2E Template Updated", 12),
        }),
      );
      expect(updatedTemplate.name).toBe("E2E Template Updated");

      expect((await api.delete(`/api/v1/attachments/templates/${templateId}`)).ok()).toBeTruthy();
    } finally {
      if (simulatorId) {
        await api.delete(`/api/v1/simulators/displays/${simulatorId}`);
      }
      await api.dispose();
    }
  });
});
