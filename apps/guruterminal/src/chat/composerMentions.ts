export type ComposerMentionKind = "skill" | "plugin";

export type ComposerPromptPart =
  | { type: "text"; value: string }
  | { type: "mention"; kind: ComposerMentionKind; value: string };

const MENTION_PATTERN = /(^|\s)([@$][\p{L}\p{N}._:@/-]*)/gu;

export const splitComposerMentions = (prompt: string): ComposerPromptPart[] => {
  const parts: ComposerPromptPart[] = [];
  let lastIndex = 0;
  for (const match of prompt.matchAll(MENTION_PATTERN)) {
    const lead = match[1] ?? "";
    const token = match[2] ?? "";
    const tokenStart = (match.index ?? 0) + lead.length;
    if (tokenStart > lastIndex) {
      parts.push({ type: "text", value: prompt.slice(lastIndex, tokenStart) });
    }
    if (token) {
      parts.push({
        type: "mention",
        kind: token.startsWith("@") ? "plugin" : "skill",
        value: token,
      });
    }
    lastIndex = tokenStart + token.length;
  }
  if (lastIndex < prompt.length) {
    parts.push({ type: "text", value: prompt.slice(lastIndex) });
  }
  return parts;
};
