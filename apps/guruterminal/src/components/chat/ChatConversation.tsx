import { FormEvent, useMemo, useState } from "react";
import {
  Conversation,
  ConversationContent,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { EmptySetupSource } from "../../app/chatOnboarding";
import type { ChatArtifactRef, ChatMessage } from "../../types";
import { ChatMessageCard } from "./ChatMessageCard";

type Props = {
  messages: ChatMessage[];
  messageKeys: string[];
  guruName: string;
  setupSources?: EmptySetupSource[];
  setupBusy?: boolean;
  setupError?: string | null;
  onSuggestion: (suggestion: string) => void;
  onOpenMarketplace?: () => void;
  onConfigureSource?: (
    entryId: string,
    config: Record<string, string>,
  ) => Promise<void>;
  onEnableSource?: (entryId: string) => Promise<void>;
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

const emptySkills = [
  {
    id: "research",
    name: "Research",
    summary: "Research and write reusable notes",
  },
  {
    id: "wiki",
    name: "Wiki",
    summary: "Write a company or industry page",
  },
  {
    id: "lens",
    name: "Lens",
    summary: "Write a thesis or lesson",
  },
  {
    id: "decision",
    name: "Decision",
    summary: "Record a judgment",
  },
] as const;

export function ChatConversation({
  messages,
  messageKeys,
  guruName,
  setupSources = [],
  setupBusy = false,
  setupError = null,
  onSuggestion,
  onOpenMarketplace,
  onConfigureSource,
  onEnableSource,
  onOpenMemory,
  onOpenInLibrary,
  onOpenArtifact,
  onOpenLink,
  onReadAttachment,
  onRevertMemory,
}: Props) {
  const [emailDrafts, setEmailDrafts] = useState<Record<string, string>>({});
  const scrollRevision = useMemo(() => JSON.stringify(messages), [messages]);
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
            <div className="chat-empty-hero">
              <h2>Ask {guruName}</h2>
              <p>Research a company, filing, or number.</p>
              <button
                type="button"
                className="chat-empty-teach"
                onClick={() => onSuggestion("$lens ")}
              >
                Set investment charter
              </button>
            </div>
            <div className="chat-empty-skills">
              {emptySkills.map((skill) => (
                <button
                  key={skill.id}
                  type="button"
                  className="chat-empty-skill"
                  aria-label={`Use $${skill.id}`}
                  onClick={() => onSuggestion(`$${skill.id} `)}
                >
                  <span className="chat-empty-skill-token">${skill.id}</span>
                  <span className="chat-empty-skill-copy">
                    <strong>{skill.name}</strong>
                    <span>{skill.summary}</span>
                  </span>
                </button>
              ))}
            </div>
            {setupSources.length ? (
              <div className="chat-empty-setup">
                {setupSources.map((source) => (
                  <EmptySetupCard
                    key={source.id}
                    source={source}
                    email={emailDrafts[source.id] ?? ""}
                    busy={setupBusy}
                    error={setupError}
                    onEmailChange={(value) =>
                      setEmailDrafts((current) => ({
                        ...current,
                        [source.id]: value,
                      }))
                    }
                    onConfigure={onConfigureSource}
                    onEnable={onEnableSource}
                    onOpenMarketplace={onOpenMarketplace}
                  />
                ))}
              </div>
            ) : null}
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

function EmptySetupCard({
  source,
  email,
  busy,
  error,
  onEmailChange,
  onConfigure,
  onEnable,
  onOpenMarketplace,
}: {
  source: EmptySetupSource;
  email: string;
  busy: boolean;
  error: string | null;
  onEmailChange: (value: string) => void;
  onConfigure?: (
    entryId: string,
    config: Record<string, string>,
  ) => Promise<void>;
  onEnable?: (entryId: string) => Promise<void>;
  onOpenMarketplace?: () => void;
}) {
  const fieldId = `empty-setup-${source.id}`;
  const submitEmail = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!source.emailField || busy) return;
    void onConfigure?.(source.id, {
      [source.emailField.id]: email.trim(),
    });
  };
  return (
    <section className="chat-empty-edgar" aria-labelledby={`${fieldId}-heading`}>
      <h3 id={`${fieldId}-heading`}>{source.name}</h3>
      <p>
        Calculations, OpenBB market data, and web research already work. US
        filings need a contact email — not an API key.
      </p>
      {source.status === "needs_enable" ? (
        <Button
          type="button"
          size="sm"
          disabled={busy}
          aria-label={`Enable ${source.name}`}
          onClick={() => void onEnable?.(source.id)}
        >
          Enable {source.name}
        </Button>
      ) : source.emailField ? (
        <form className="chat-empty-edgar-form" onSubmit={submitEmail}>
          <Label htmlFor={fieldId}>{source.emailField.label}</Label>
          <div className="chat-empty-edgar-row">
            <Input
              id={fieldId}
              type="email"
              autoComplete="email"
              required
              disabled={busy}
              value={email}
              onChange={(event) => onEmailChange(event.target.value)}
            />
            <Button type="submit" size="sm" disabled={busy}>
              Save
            </Button>
          </div>
        </form>
      ) : null}
      {error ? (
        <p className="chat-empty-edgar-error" role="alert">
          {error}
        </p>
      ) : null}
      {onOpenMarketplace ? (
        <button
          type="button"
          className="chat-empty-setup-link"
          onClick={onOpenMarketplace}
        >
          Other sources
        </button>
      ) : null}
    </section>
  );
}
