import type { ChatMessage } from "../types";

export type VisibleSteer = {
  id: string;
  text: string;
  createdAt: string;
};

/** Insert delivered steers into the visible transcript if the live Chat dropped them. */
export function withVisibleSteers(
  messages: ChatMessage[],
  messageKeys: string[],
  steers: VisibleSteer[],
): { messages: ChatMessage[]; messageKeys: string[] } {
  if (steers.length === 0) return { messages, messageKeys };
  const existing = new Set(messages.map((message) => message.id));
  const incoming = steers.filter((item) => !existing.has(item.id));
  if (incoming.length === 0) return { messages, messageKeys };
  const insertAt =
    messages.at(-1)?.role === "assistant" ? messages.length - 1 : messages.length;
  return {
    messages: [
      ...messages.slice(0, insertAt),
      ...incoming.map((item) => ({
        id: item.id,
        role: "user" as const,
        content: item.text,
        created_at: item.createdAt,
        status: "complete" as const,
      })),
      ...messages.slice(insertAt),
    ],
    messageKeys: [
      ...messageKeys.slice(0, insertAt),
      ...incoming.map((item) => item.id),
      ...messageKeys.slice(insertAt),
    ],
  };
}
