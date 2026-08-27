import {
  AtSignIcon,
  ClipboardIcon,
  CommandIcon,
  CornerDownLeftIcon,
  FileIcon,
  ListPlusIcon,
  PaperclipIcon,
  SquareIcon,
  WorkflowIcon,
  XIcon,
} from "lucide-react";
import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type ClipboardEvent,
  type KeyboardEvent,
  type Ref,
} from "react";
import { splitComposerMentions } from "../../chat/composerMentions";
import type { FileUIPart } from "ai";
import {
  PromptInput,
  PromptInputActionAddAttachments,
  PromptInputActionMenu,
  PromptInputActionMenuContent,
  PromptInputActionMenuItem,
  PromptInputActionMenuTrigger,
  PromptInputBody,
  PromptInputButton,
  PromptInputFooter,
  PromptInputSubmit,
  PromptInputTextarea,
  PromptInputTools,
  usePromptInputAttachments,
} from "@/components/ai-elements/prompt-input";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type {
  ConfiguredModel,
  ModelProviderOption,
  ModelRunSelection,
} from "../../types";
import { cn } from "@/lib/utils";
import { ChatModelMenu } from "./ChatModelMenu";

export type ComposerSkillOption = {
  id: string;
  name: string;
  description: string;
};

export type ComposerPluginOption = {
  id: string;
  name: string;
  description: string;
};

type TriggerSymbol = "@" | "$" | "/";

type ActiveTrigger = {
  symbol: TriggerSymbol;
  query: string;
  start: number;
  end: number;
};

type AssistKind = "skill" | "plugin";

type AssistOption = {
  id: string;
  kind: AssistKind;
  label: string;
  detail: string;
  insert: string;
};

const skillAssistOptions = (skills: ComposerSkillOption[]): AssistOption[] =>
  skills.map((skill) => ({
    id: `skill:${skill.id}`,
    kind: "skill",
    label: `$${skill.id}`,
    detail: skill.description,
    insert: `$${skill.id}`,
  }));

const pluginAssistOptions = (
  plugins: ComposerPluginOption[],
): AssistOption[] =>
  plugins.map((plugin) => ({
    id: `plugin:${plugin.id}`,
    kind: "plugin",
    label: `@${plugin.id}`,
    detail: plugin.description,
    insert: `@${plugin.id}`,
  }));

const assistHeading = (symbol: TriggerSymbol) => {
  if (symbol === "@") return "Plugins";
  if (symbol === "$") return "Skills";
  return "Skills & plugins";
};

function ComposerPromptHighlight({
  prompt,
  highlightRef,
}: {
  prompt: string;
  highlightRef: Ref<HTMLDivElement>;
}) {
  const parts = splitComposerMentions(prompt);
  return (
    <div
      ref={highlightRef}
      className="composer-prompt-highlight"
      aria-hidden="true"
    >
      {parts.map((part, index) =>
        part.type === "mention" ? (
          <span key={index} data-mention={part.kind}>
            {part.value}
          </span>
        ) : (
          <span key={index}>{part.value}</span>
        ),
      )}
      {"\n"}
    </div>
  );
}

const activeTriggerAt = (
  value: string,
  cursor: number,
): ActiveTrigger | null => {
  const prefix = value.slice(0, cursor);
  const match = prefix.match(/(?:^|\s)([@$/])([\p{L}\p{N}._:@/-]*)$/u);
  if (!match) return null;
  const symbol = match[1] as TriggerSymbol;
  const query = match[2] ?? "";
  return {
    symbol,
    query,
    start: cursor - query.length - 1,
    end: cursor,
  };
};

type Props = {
  prompt: string;
  useMemory: boolean;
  updateMemory: boolean;
  memoryControlsLocked?: boolean;
  isRunning: boolean;
  isStopping?: boolean;
  error: string | null;
  streamAnnouncement: string;
  models: ConfiguredModel[];
  providers: ModelProviderOption[];
  modelSelection: ModelRunSelection;
  skills: ComposerSkillOption[];
  plugins: ComposerPluginOption[];
  onPromptChange: (prompt: string) => void;
  onUseMemoryChange: (enabled: boolean) => void;
  onUpdateMemoryChange: (enabled: boolean) => void;
  onModelSelectionChange: (selection: ModelRunSelection) => void;
  onSend: (prompt: string, files: FileUIPart[]) => void;
  onFollowUp: () => void;
  onAbort: () => void;
};

function ComposerAttachmentTray() {
  const attachments = usePromptInputAttachments();
  if (attachments.files.length === 0) return null;

  return (
    <div className="composer-attachments" aria-label="Message attachments">
      {attachments.files.map((file) => (
        <div className="composer-attachment" key={file.id}>
          {file.mediaType.startsWith("image/") ? (
            <img src={file.url} alt="" />
          ) : (
            <FileIcon aria-hidden="true" />
          )}
          <span>
            <strong>{file.filename ?? "Attachment"}</strong>
            <small>{file.mediaType || "File"}</small>
          </span>
          <button
            type="button"
            aria-label={`Remove ${file.filename ?? "attachment"}`}
            onClick={() => attachments.remove(file.id)}
          >
            <XIcon />
          </button>
        </div>
      ))}
    </div>
  );
}

function ClipboardAttachmentAction({
  onError,
}: {
  onError: (message: string) => void;
}) {
  const attachments = usePromptInputAttachments();

  const attachClipboard = async () => {
    if (!navigator.clipboard?.read) {
      onError("Clipboard attachments are not available in this environment.");
      return;
    }
    try {
      const items = await navigator.clipboard.read();
      const files: File[] = [];
      for (const item of items) {
        for (const mediaType of item.types) {
          const blob = await item.getType(mediaType);
          if (!mediaType.startsWith("image/")) continue;
          const extension =
            mediaType === "image/png"
              ? "png"
              : mediaType === "image/jpeg"
                ? "jpg"
                : mediaType === "image/webp"
                  ? "webp"
                  : mediaType === "image/gif"
                    ? "gif"
                    : "img";
          files.push(
            new File([blob], `clipboard-${Date.now()}.${extension}`, {
              type: mediaType,
            }),
          );
        }
      }
      if (files.length === 0) {
        onError("The clipboard does not contain an attachable image.");
        return;
      }
      attachments.add(files);
    } catch (cause) {
      if (cause instanceof DOMException && cause.name === "NotAllowedError") {
        onError("Clipboard access was not allowed.");
      } else {
        onError("Could not attach clipboard contents.");
      }
    }
  };

  return (
    <PromptInputActionMenuItem
      onSelect={(event) => {
        event.preventDefault();
        void attachClipboard();
      }}
    >
      <ClipboardIcon />
      Paste image from clipboard
    </PromptInputActionMenuItem>
  );
}

function ComposerMemoryToggle({
  label,
  title,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  title: string;
  checked: boolean;
  disabled: boolean;
  onChange: (enabled: boolean) => void;
}) {
  return (
    <label className="composer-checkbox" title={title}>
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>{label}</span>
    </label>
  );
}

function ComposerPromptTools({
  useMemory,
  updateMemory,
  memoryControlsLocked,
  models,
  providers,
  modelSelection,
  onUseMemoryChange,
  onUpdateMemoryChange,
  onModelSelectionChange,
  onAttachmentError,
}: {
  useMemory: boolean;
  updateMemory: boolean;
  memoryControlsLocked: boolean;
  models: ConfiguredModel[];
  providers: ModelProviderOption[];
  modelSelection: ModelRunSelection;
  onUseMemoryChange: (enabled: boolean) => void;
  onUpdateMemoryChange: (enabled: boolean) => void;
  onModelSelectionChange: (selection: ModelRunSelection) => void;
  onAttachmentError: (message: string) => void;
}) {
  return (
    <PromptInputTools className="composer-tools">
      <PromptInputActionMenu>
        <PromptInputActionMenuTrigger
          aria-label="Attach files or clipboard content"
          tooltip="Attach files, images, or clipboard content"
        >
          <PaperclipIcon />
        </PromptInputActionMenuTrigger>
        <PromptInputActionMenuContent>
          <PromptInputActionAddAttachments label="Add files or images" />
          <ClipboardAttachmentAction onError={onAttachmentError} />
        </PromptInputActionMenuContent>
      </PromptInputActionMenu>
      <ChatModelMenu
        models={models}
        providers={providers}
        selection={modelSelection}
        onSelectionChange={onModelSelectionChange}
      />
      <ComposerMemoryToggle
        label="Use memory"
        title={
          memoryControlsLocked
            ? "This skill always uses Memory"
            : "Reuse this Guru's saved facts and lessons"
        }
        checked={useMemory}
        disabled={memoryControlsLocked}
        onChange={onUseMemoryChange}
      />
      <ComposerMemoryToggle
        label="Update memory"
        title={
          memoryControlsLocked
            ? "This skill can save a short Memory update"
            : "Let this Guru keep reusable facts and lessons"
        }
        checked={updateMemory}
        disabled={memoryControlsLocked}
        onChange={onUpdateMemoryChange}
      />
    </PromptInputTools>
  );
}

function ComposerRunActions({
  canSend,
  onQueue,
  onSteer,
  onAbort,
  onPreserveFocus,
}: {
  canSend: boolean;
  onQueue: () => void;
  onSteer: () => void;
  onAbort: () => void;
  onPreserveFocus: (event: { preventDefault: () => void }) => void;
}) {
  return (
    <div className="composer-run-actions">
      <PromptInputButton
        aria-label="Queue after current response"
        className="composer-queue-action"
        disabled={!canSend}
        size="sm"
        tooltip={{
          content: "Send after this response",
          shortcut: "↵",
        }}
        variant="default"
        onMouseDown={onPreserveFocus}
        onClick={onQueue}
      >
        <ListPlusIcon />
        <span className="composer-run-action-label">Queue</span>
      </PromptInputButton>
      <PromptInputButton
        aria-label="Steer current response"
        className="composer-steer-action"
        disabled={!canSend}
        tooltip={{
          content: "Guide this response now",
          shortcut: "⌘↵",
        }}
        onMouseDown={onPreserveFocus}
        onClick={onSteer}
      >
        <CornerDownLeftIcon />
        <span className="composer-run-action-label">Steer</span>
      </PromptInputButton>
      <PromptInputButton
        aria-label="Stop response"
        className="composer-stop-action"
        tooltip="Stop response"
        onClick={onAbort}
      >
        <SquareIcon />
      </PromptInputButton>
    </div>
  );
}

function ComposerSubmit({
  prompt,
  hasModel,
  disabledReason,
}: {
  prompt: string;
  hasModel: boolean;
  disabledReason?: string;
}) {
  const attachments = usePromptInputAttachments();
  const empty = !prompt.trim() && attachments.files.length === 0;
  return (
    <PromptInputSubmit
      aria-label="Send"
      status="ready"
      disabled={empty || !hasModel}
      title={!hasModel ? disabledReason : undefined}
    />
  );
}

export function ChatComposer({
  prompt,
  useMemory,
  updateMemory,
  memoryControlsLocked = false,
  isRunning,
  isStopping = false,
  error,
  streamAnnouncement,
  models,
  providers,
  modelSelection,
  skills,
  plugins,
  onPromptChange,
  onUseMemoryChange,
  onUpdateMemoryChange,
  onModelSelectionChange,
  onSend,
  onFollowUp,
  onAbort,
}: Props) {
  const [attachmentError, setAttachmentError] = useState<string | null>(null);
  const [activeTrigger, setActiveTrigger] = useState<ActiveTrigger | null>(
    null,
  );
  const [activeOption, setActiveOption] = useState(0);
  const [isComposing, setIsComposing] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const highlightRef = useRef<HTMLDivElement>(null);
  const selectedModel = models.find(
    (model) => model.id === modelSelection.model_profile_id,
  );
  const assistOptions = useMemo<AssistOption[]>(() => {
    if (!activeTrigger) return [];
    const query = activeTrigger.query.toLocaleLowerCase("en-US");
    const skillsOptions = skillAssistOptions(skills);
    const pluginsOptions = pluginAssistOptions(plugins);
    const options =
      activeTrigger.symbol === "@"
        ? pluginsOptions
        : activeTrigger.symbol === "$"
          ? skillsOptions
          : [...skillsOptions, ...pluginsOptions];
    return options
      .filter((option) =>
        `${option.id} ${option.label} ${option.detail}`
          .toLocaleLowerCase("en-US")
          .includes(query),
      )
      .slice(0, 12);
  }, [activeTrigger, plugins, skills]);

  useEffect(() => {
    setActiveOption(0);
  }, [activeTrigger?.query, activeTrigger?.symbol]);

  const syncTrigger = (target: HTMLTextAreaElement) => {
    setActiveTrigger(activeTriggerAt(target.value, target.selectionStart));
  };

  const syncHighlightScroll = (target: HTMLTextAreaElement) => {
    const highlight = highlightRef.current;
    if (!highlight) return;
    highlight.scrollTop = target.scrollTop;
    highlight.scrollLeft = target.scrollLeft;
  };

  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (textarea) syncHighlightScroll(textarea);
  }, [prompt]);

  const selectAssist = (option: AssistOption) => {
    if (!activeTrigger) return;
    const before = prompt.slice(0, activeTrigger.start);
    const after = prompt.slice(activeTrigger.end);
    const next = `${before}${option.insert} ${after}`;
    const cursor = before.length + option.insert.length + 1;
    onPromptChange(next);
    setActiveTrigger(null);
    requestAnimationFrame(() => {
      textareaRef.current?.focus();
      textareaRef.current?.setSelectionRange(cursor, cursor);
    });
  };

  const handlePromptChange = (event: ChangeEvent<HTMLTextAreaElement>) => {
    onPromptChange(event.target.value);
    syncTrigger(event.target);
  };

  const blockFilePasteWhileRunning = (
    event: ClipboardEvent<HTMLTextAreaElement>,
  ) => {
    const items = event.clipboardData?.items;
    if (items && [...items].some((item) => item.kind === "file")) {
      event.preventDefault();
    }
  };

  const keepComposerFocus = () => {
    textareaRef.current?.focus();
  };

  const preserveComposerFocus = (
    event: { preventDefault: () => void },
  ) => {
    event.preventDefault();
  };

  const queueFollowUp = () => {
    if (!prompt.trim()) return;
    onFollowUp();
    keepComposerFocus();
  };

  const steerCurrent = () => {
    const text = prompt.trim();
    if (!text) return;
    onSend(text, []);
    keepComposerFocus();
  };

  const handlePromptKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (isRunning && event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      if (event.metaKey || event.ctrlKey) {
        steerCurrent();
      } else {
        queueFollowUp();
      }
      return;
    }
    if (!activeTrigger) return;
    if (event.key === "Escape") {
      event.preventDefault();
      setActiveTrigger(null);
      return;
    }
    if (assistOptions.length === 0) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveOption((current) => (current + 1) % assistOptions.length);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveOption(
        (current) =>
          (current - 1 + assistOptions.length) % assistOptions.length,
      );
      return;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      selectAssist(assistOptions[activeOption] ?? assistOptions[0]);
    }
  };

  return (
    <div className="composer-shell" aria-busy={isRunning}>
      <span className="sr-only" role="status" aria-live="polite">
        {streamAnnouncement}
      </span>

      {(error || attachmentError) && (
        <Alert variant="destructive" className="composer-error" role="alert">
          <AlertDescription>{error ?? attachmentError}</AlertDescription>
        </Alert>
      )}

      {activeTrigger && (
        <div
          className="composer-assist"
          role="listbox"
          aria-label="Prompt shortcuts"
        >
          <div className="composer-assist-heading">
            {activeTrigger.symbol === "@" ? (
              <AtSignIcon />
            ) : activeTrigger.symbol === "$" ? (
              <WorkflowIcon />
            ) : (
              <CommandIcon />
            )}
            <span>{assistHeading(activeTrigger.symbol)}</span>
          </div>
          {assistOptions.length === 0 ? (
            <p>No matching item</p>
          ) : (
            assistOptions.map((option, index) => (
              <button
                key={option.id}
                type="button"
                role="option"
                aria-selected={index === activeOption}
                onMouseDown={(event) => event.preventDefault()}
                onClick={() => selectAssist(option)}
                onMouseEnter={() => setActiveOption(index)}
              >
                <span className="composer-assist-icon">
                  {option.kind === "plugin" ? (
                    <AtSignIcon />
                  ) : (
                    <WorkflowIcon />
                  )}
                </span>
                <span>
                  <strong>
                    <span data-mention={option.kind}>{option.label}</span>
                  </strong>
                  <small>{option.detail}</small>
                </span>
              </button>
            ))
          )}
          <footer>
            <kbd>↑↓</kbd> navigate <kbd>Enter</kbd> select <kbd>Esc</kbd> close
          </footer>
        </div>
      )}

      <PromptInput
        className={cn(
          "composer",
          isRunning && "composer-running",
          isStopping && "composer-stopping",
        )}
        multiple
        maxFiles={4}
        maxFileSize={20 * 1024 * 1024}
        onError={({ message }) => setAttachmentError(message)}
        onSubmit={({ text, files }) => {
          const next = text.trim();
          if (!next && files.length === 0) return false;
          if (isRunning) {
            if (!next) return false;
            setAttachmentError(null);
            onFollowUp();
            keepComposerFocus();
            return true;
          }
          if (
            files.some((file) => file.mediaType.startsWith("image/")) &&
            !selectedModel?.input.includes("image")
          ) {
            setAttachmentError("The selected model does not accept images.");
            return false;
          }
          if (
            files.some(
              (file) =>
                file.mediaType.startsWith("image/") &&
                ("size" in file &&
                  typeof file.size === "number" &&
                  file.size > 5 * 1024 * 1024),
            )
          ) {
            setAttachmentError("Image attachments must be 5 MB or smaller.");
            return false;
          }
          setAttachmentError(null);
          onSend(next, files);
          return true;
        }}
      >
        <ComposerAttachmentTray />
        <PromptInputBody>
          <div
            className={cn("composer-prompt", isComposing && "is-composing")}
            onCompositionStart={() => setIsComposing(true)}
            onCompositionEnd={() => setIsComposing(false)}
          >
            <ComposerPromptHighlight
              prompt={prompt}
              highlightRef={highlightRef}
            />
            <PromptInputTextarea
              ref={textareaRef}
              aria-label="Message Guru"
              placeholder="Ask Guru"
              value={prompt}
              onChange={handlePromptChange}
              onClick={(event) => syncTrigger(event.currentTarget)}
              onScroll={(event) => syncHighlightScroll(event.currentTarget)}
              {...(isRunning ? { onPaste: blockFilePasteWhileRunning } : {})}
              onKeyDown={handlePromptKeyDown}
              onKeyUp={(event) => {
                if (
                  ["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)
                ) {
                  syncTrigger(event.currentTarget);
                }
              }}
            />
          </div>
        </PromptInputBody>
        <PromptInputFooter className="composer-footer">
          <ComposerPromptTools
            useMemory={useMemory}
            updateMemory={updateMemory}
            memoryControlsLocked={memoryControlsLocked}
            models={models}
            providers={providers}
            modelSelection={modelSelection}
            onUseMemoryChange={onUseMemoryChange}
            onUpdateMemoryChange={onUpdateMemoryChange}
            onModelSelectionChange={onModelSelectionChange}
            onAttachmentError={setAttachmentError}
          />
          {isRunning ? (
            <ComposerRunActions
              canSend={Boolean(prompt.trim())}
              onQueue={queueFollowUp}
              onSteer={steerCurrent}
              onAbort={onAbort}
              onPreserveFocus={preserveComposerFocus}
            />
          ) : (
            <ComposerSubmit
              prompt={prompt}
              hasModel={Boolean(
                modelSelection.model_profile_id &&
                modelSelection.thinking_level,
              )}
              disabledReason={
                modelSelection.model_profile_id
                  ? "Choose a thinking level"
                  : "Choose a model"
              }
            />
          )}
        </PromptInputFooter>
      </PromptInput>
    </div>
  );
}
