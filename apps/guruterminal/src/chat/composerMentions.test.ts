import { describe, expect, it } from "vitest";
import { splitComposerMentions } from "./composerMentions";

describe("splitComposerMentions", () => {
  it("keeps ordinary text unhighlighted", () => {
    expect(splitComposerMentions("Samsung earnings")).toEqual([
      { type: "text", value: "Samsung earnings" },
    ]);
  });

  it("marks $ Skills and @ plugins after a start or space", () => {
    expect(
      splitComposerMentions("$research hello @sec.edgar next"),
    ).toEqual([
      { type: "mention", kind: "skill", value: "$research" },
      { type: "text", value: " hello " },
      { type: "mention", kind: "plugin", value: "@sec.edgar" },
      { type: "text", value: " next" },
    ]);
  });

  it("does not treat an email local-part as a plugin mention", () => {
    expect(splitComposerMentions("write research@example.com")).toEqual([
      { type: "text", value: "write research@example.com" },
    ]);
  });

  it("highlights an in-progress $ or @ trigger", () => {
    expect(splitComposerMentions("$res")).toEqual([
      { type: "mention", kind: "skill", value: "$res" },
    ]);
    expect(splitComposerMentions(" @")).toEqual([
      { type: "text", value: " " },
      { type: "mention", kind: "plugin", value: "@" },
    ]);
  });
});
