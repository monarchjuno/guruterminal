import {
  createGuruTerminalBridge,
  MockGuruTerminalBridge,
  TAURI_COMMANDS,
  TAURI_STREAM_CHANNEL_ARGUMENT,
} from "./bridge";
import type { ChatStreamEvent } from "./types";

describe("Guru Terminal bridge contract", () => {
  it("defaults production and unconfigured builds to the native bridge", () => {
    delete document.documentElement.dataset.guruTerminalBackend;
    expect(createGuruTerminalBridge().constructor.name).toBe(
      "TauriGuruTerminalBridge",
    );
    expect(
      document.documentElement.dataset.guruTerminalBackend,
    ).toBeUndefined();
  });

  it("pins the snake_case Tauri command and channel names", () => {
    expect(Object.values(TAURI_COMMANDS)).toEqual([
      "model_catalog_get",
      "model_visibility_update",
      "provider_models",
      "provider_configure",
      "provider_connect",
      "provider_connect_cancel",
      "provider_connect_open_browser",
      "provider_disconnect",
      "marketplace_snapshot",
      "guru_capability_list",
      "agent_skill_catalog",
      "agent_skills_update",
      "guru_capability_enable",
      "marketplace_connector_configure",
      "guru_capability_disable",
      "marketplace_credential_save",
      "marketplace_credential_verify",
      "marketplace_credential_delete",
      "open_external_url",
      "browser_tab_open",
      "browser_tab_navigate",
      "browser_tab_history",
      "browser_tab_reload",
      "browser_tab_set_bounds",
      "browser_tab_close",
      "browser_tabs_reset",
      "update_status",
      "update_check",
      "update_install",
      "guru_list",
      "guru_select",
      "guru_recover",
      "guru_create",
      "guru_import_memory",
      "guru_export_memory",
      "guru_rename",
      "guru_delete",
      "chat_create",
      "chat_rename",
      "chat_delete",
      "chat_attachment_read",
      "chat_artifact_list",
      "chat_artifact_read",
      "chat_send",
      "chat_steer",
      "chat_abort",
      "run_activity_list",
      "library_search",
      "library_read",
      "library_memory_create",
      "library_memory_update",
      "library_memory_delete",
      "library_memory_revert",
    ]);
    expect(TAURI_STREAM_CHANNEL_ARGUMENT).toBe("on_event");
  });

  it("keeps Agent skills deterministic and removes deleted Agent state", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });

    expect(await bridge.agentSkillCatalog("guru-quality")).toEqual([
      expect.objectContaining({
        id: "research",
        enabled: true,
      }),
      expect.objectContaining({
        id: "wiki",
        enabled: true,
      }),
      expect.objectContaining({
        id: "lens",
        enabled: true,
      }),
    ]);

    const updated = await bridge.agentSkillsUpdate({
      guru_id: "guru-quality",
      skill_ids: ["wiki", "research"],
    });
    expect(updated.enabled_skill_ids).toEqual([
      "research",
      "wiki",
    ]);
    expect(
      (await bridge.agentSkillCatalog("guru-quality")).find(
        (skill) => skill.id === "lens",
      ),
    ).toMatchObject({ enabled: false });

    await bridge.guruDelete({ guru_id: "guru-quality" });
    expect(
      (await bridge.guruList()).some((guru) => guru.id === "guru-quality"),
    ).toBe(false);
    await expect(bridge.guruSelect("guru-quality")).rejects.toThrow(
      /not found/i,
    );
  });

  it("keeps the mock external-link boundary aligned with the native host", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const open = vi.spyOn(window, "open").mockImplementation(() => null);
    const dart = "https://dart.fss.or.kr/dsab001/search.ax?textCrpNm=005930";

    await bridge.openExternalUrl(dart);
    expect(open).toHaveBeenCalledWith(dart, "_blank", "noopener,noreferrer");
    await expect(bridge.openExternalUrl("file:///tmp/private")).rejects.toThrow(
      /HTTP and HTTPS/i,
    );
    await expect(
      bridge.openExternalUrl("https://user:secret@example.com/report"),
    ).rejects.toThrow(/HTTP and HTTPS/i);
    await expect(
      bridge.openExternalUrl(`https://example.com/${"a".repeat(8 * 1024)}`),
    ).rejects.toThrow(/too long/i);
    expect(open).toHaveBeenCalledTimes(1);
    open.mockRestore();
  });

  it("models browser tabs without loading remote content into the renderer", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const events: Array<{ type: string }> = [];
    const bounds = { x: 12, y: 24, width: 640, height: 480 };

    const first = await bridge.browserTabOpen(
      { url: "https://example.com/report", bounds, visible: true },
      (event) => events.push(event),
    );
    const second = await bridge.browserTabOpen(
      { url: "https://example.com/report", bounds, visible: false },
      (event) => events.push(event),
    );

    expect(first.tab_id).not.toBe(second.tab_id);
    expect(first).toMatchObject({
      url: "https://example.com/report",
      title: "example.com",
    });
    await bridge.browserTabNavigate(first.tab_id, "https://example.com/next");
    await bridge.browserTabHistory(first.tab_id, "back");
    await bridge.browserTabReload(first.tab_id);
    await bridge.browserTabSetBounds({
      tab_id: first.tab_id,
      bounds: { ...bounds, width: 720 },
      visible: true,
    });
    await Promise.resolve();
    expect(events.some((event) => event.type === "load_started")).toBe(true);
    expect(events.some((event) => event.type === "load_finished")).toBe(true);

    await bridge.browserTabClose(first.tab_id);
    await expect(bridge.browserTabReload(first.tab_id)).rejects.toThrow(
      /not found/i,
    );
    await expect(
      bridge.browserTabOpen(
        { url: "file:///tmp/private", bounds, visible: true },
        () => undefined,
      ),
    ).rejects.toThrow(/HTTP and HTTPS/i);
    await bridge.browserTabsReset();
    await expect(bridge.browserTabReload(second.tab_id)).rejects.toThrow(
      /not found/i,
    );
  });

  it("updates memory independently when Use memory is off", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const events: ChatStreamEvent[] = [];

    await new Promise<void>(async (resolve, reject) => {
      try {
        await bridge.chatSend(
          {
            run_id: "chat-proposal-no-memory",
            guru_id: "guru-quality",
            model_profile_id: "model-test",
            thread_id: "thread-margin",
            prompt: "Teach this once",
            use_memory: false,
            update_memory: true,
            thinking_level: "medium",
            run_options: {},
            attachments: [],
          },
          (event) => {
            events.push(event);
            if (event.type === "completed") resolve();
            if (event.type === "error") reject(new Error(event.message));
          },
        );
      } catch (error) {
        reject(error);
      }
    });

    expect(events.some((event) => event.type === "memory")).toBe(false);
    expect(events.some((event) => event.type === "memory_update")).toBe(true);
    expect(events.some((event) => event.type === "progress")).toBe(false);
    expect(events.at(-1)?.type).toBe("completed");
    const completedEvent = events.find((event) => event.type === "completed");
    expect(
      completedEvent?.type === "completed"
        ? completedEvent.agent_harness
        : undefined,
    ).toMatchObject({
      schema: "guruterminal-harness/1",
      mode: "chat",
      skill_ids: ["research", "wiki", "lens"],
      capability_ids: expect.arrayContaining(["guruterminal.finance-core"]),
      digest: expect.any(String),
    });
    const updateEvent = events.find((event) => event.type === "memory_update");
    expect(
      updateEvent?.type === "memory_update"
        ? Object.keys(updateEvent.result).sort()
        : undefined,
    ).toEqual(["changes", "commitId", "status"].sort());

    const workspace = await bridge.guruSelect("guru-quality");
    const persisted = workspace.threads
      .find((thread) => thread.id === "thread-margin")
      ?.messages.find((message) => message.memory_update)?.memory_update;
    expect(persisted).toEqual(
      updateEvent?.type === "memory_update" ? updateEvent.result : undefined,
    );
    const persistedMessage = workspace.threads
      .find((thread) => thread.id === "thread-margin")
      ?.messages.find((message) => message.memory_update);
    expect(persistedMessage?.agent_harness).toEqual(
      completedEvent?.type === "completed"
        ? completedEvent.agent_harness
        : undefined,
    );
  });

  it("names a new Chat thread and persists the generated title", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const thread = await bridge.chatCreate({ guru_id: "guru-quality" });
    const events: ChatStreamEvent[] = [];

    await new Promise<void>(async (resolve, reject) => {
      try {
        await bridge.chatSend(
          {
            run_id: "chat-generated-title",
            guru_id: "guru-quality",
            model_profile_id: "model-test",
            thread_id: thread.id,
            prompt: "삼성전자 실적 분석해봐",
            use_memory: true,
            update_memory: false,
            thinking_level: "medium",
            run_options: {},
            attachments: [],
          },
          (event) => {
            events.push(event);
            if (event.type === "completed") resolve();
            if (event.type === "error") reject(new Error(event.message));
          },
        );
      } catch (error) {
        reject(error);
      }
    });

    expect(events).toContainEqual({
      type: "title",
      run_id: expect.any(String),
      title: "삼성전자 실적 분석해봐",
    });
    const workspace = await bridge.guruSelect("guru-quality");
    expect(workspace.threads.find((item) => item.id === thread.id)?.title).toBe(
      "삼성전자 실적 분석해봐",
    );
  });

  it("renames and deletes a Chat thread within its Guru", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const created = await bridge.chatCreate({ guru_id: "guru-quality" });

    const renamed = await bridge.chatRename({
      guru_id: "guru-quality",
      thread_id: created.id,
      title: "Durable session name",
    });
    expect(renamed.title).toBe("Durable session name");

    await bridge.chatDelete({
      guru_id: "guru-quality",
      thread_id: created.id,
    });
    expect(
      (await bridge.guruSelect("guru-quality")).threads.some(
        (thread) => thread.id === created.id,
      ),
    ).toBe(false);
  });

  it("persists an attachment-only message and reads its exact bytes", async () => {
    const bridge = new MockGuruTerminalBridge({ delay_ms: 0 });
    const thread = await bridge.chatCreate({ guru_id: "guru-quality" });
    const data_base64 = "Ym91bmRlZCBhdHRhY2htZW50";

    await new Promise<void>((resolve, reject) => {
      void bridge
        .chatSend(
          {
            run_id: "chat-attachment-only",
            guru_id: "guru-quality",
            model_profile_id: "model-test",
            thread_id: thread.id,
            prompt: "",
            use_memory: false,
            update_memory: false,
            thinking_level: "medium",
            run_options: {},
            attachments: [
              {
                filename: "notes.txt",
                media_type: "text/plain",
                data_base64,
              },
            ],
          },
          (event) => {
            if (event.type === "completed") resolve();
            if (event.type === "error") reject(new Error(event.message));
          },
        )
        .catch(reject);
    });

    const stored = (await bridge.guruSelect("guru-quality")).threads
      .find((item) => item.id === thread.id)
      ?.messages.find((message) => message.role === "user");
    expect(stored).toMatchObject({
      content: "",
      attachments: [
        {
          filename: "notes.txt",
          media_type: "text/plain",
        },
      ],
    });
    const attachment = stored?.attachments?.[0];
    expect(attachment).toBeDefined();
    await expect(
      bridge.chatAttachmentRead(
        "guru-quality",
        thread.id,
        stored!.id,
        attachment!.id,
      ),
    ).resolves.toEqual({
      data_url: `data:text/plain;base64,${data_base64}`,
    });
  });

});
