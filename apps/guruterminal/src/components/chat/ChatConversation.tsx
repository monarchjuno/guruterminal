import { useMemo } from "react";
import {
  Conversation,
  ConversationContent,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import type { ChatArtifactRef, ChatMessage } from "../../types";
import { ChatMessageCard } from "./ChatMessageCard";

type Props = {
  messages: ChatMessage[];
  messageKeys: string[];
  guruName: string;
  onSuggestion: (suggestion: string) => void;
  onOpenMemory: (recordId: string, title: string) => void;
  onOpenInLibrary: (recordId: string) => void;
  onOpenArtifact: (artifact: ChatArtifactRef) => void;
  onOpenLink: (url: string) => Promise<void>;
  onReadAttachment: (
    messageId: string,
    attachmentId: string,
  ) => Promise<string>;
  onRevertMemory: (recordId: string, commitId: string) => Promise<void>;
};

const emptySkills = ["research", "wiki", "lens", "decision"] as const;

export function ChatConversation({
  messages,
  messageKeys,
  guruName,
  onSuggestion,
  onOpenMemory,
  onOpenInLibrary,
  onOpenArtifact,
  onOpenLink,
  onReadAttachment,
  onRevertMemory,
}: Props) {
  const scrollRevision = useMemo(() => {
    const last = messages.at(-1);
    return [
      messages.length,
      messageKeys.at(-1) ?? "",
      last?.status ?? "",
      last?.content.length ?? 0,
      last?.progress?.items.length ?? 0,
    ].join(":");
  }, [messageKeys, messages]);
  return (
    <Conversation
      className="message-scroll"
      aria-label="Conversation"
      aria-relevant="additions"
      scrollRevision={scrollRevision}
    >
      <ConversationContent className="conversation-content">
        {!messages.length ? (
          <div className="chat-empty">
            <h2>Ask {guruName}</h2>
            <div className="chat-empty-skills">
              {emptySkills.map((skill) => (
                <button
                  key={skill}
                  type="button"
                  className="chat-empty-skill"
                  aria-label={`Use $${skill}`}
                  onClick={() => onSuggestion(`$${skill} `)}
                >
                  ${skill}
                </button>
              ))}
            </div>
          </div>
        ) : (
          messages.map((message, index) => (
            <ChatMessageCard
              key={messageKeys[index] ?? message.id}
              message={message}
              guruName={guruName}
              onOpenMemory={onOpenMemory}
              onOpenInLibrary={onOpenInLibrary}
              onOpenArtifact={onOpenArtifact}
              onOpenLink={onOpenLink}
              onReadAttachment={onReadAttachment}
              onRevertMemory={onRevertMemory}
            />
          ))
        )}
      </ConversationContent>
      <ConversationScrollButton />
    </Conversation>
  );
}
