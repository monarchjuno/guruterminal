import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { ArrowDownIcon } from "lucide-react";
import {
  createContext,
  type ComponentProps,
  type UIEvent,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

type ConversationContextValue = {
  isAtBottom: boolean;
  scrollToBottom: () => void;
};

const ConversationContext = createContext<ConversationContextValue | null>(null);
const BOTTOM_TOLERANCE_PX = 2;

export type ConversationProps = ComponentProps<"div"> & {
  /** Advances when rendered Chat content changes and should stay in view. */
  scrollRevision?: string | number;
};

/**
 * A deliberately small scroll owner for high-frequency Chat updates.
 *
 * Its DOM writes never update React state. This avoids ResizeObserver-driven
 * render feedback while an assistant is streaming many small progress events.
 */
export const Conversation = ({
  className,
  children,
  onScroll,
  role = "log",
  scrollRevision,
  ...props
}: ConversationProps) => {
  const scrollRef = useRef<HTMLDivElement>(null);
  const isAtBottomRef = useRef(true);
  const [isAtBottom, setIsAtBottom] = useState(true);

  const updateIsAtBottom = useCallback((next: boolean) => {
    if (isAtBottomRef.current === next) return;
    isAtBottomRef.current = next;
    setIsAtBottom(next);
  }, []);

  const scrollToBottom = useCallback(() => {
    const element = scrollRef.current;
    if (!element) return;
    element.scrollTop = element.scrollHeight;
    updateIsAtBottom(true);
  }, [updateIsAtBottom]);

  useLayoutEffect(() => {
    if (isAtBottomRef.current) scrollToBottom();
  }, [scrollRevision, scrollToBottom]);

  const handleScroll = useCallback(
    (event: UIEvent<HTMLDivElement>) => {
      const element = event.currentTarget;
      updateIsAtBottom(
        element.scrollHeight - element.scrollTop - element.clientHeight <=
          BOTTOM_TOLERANCE_PX,
      );
      onScroll?.(event);
    },
    [onScroll, updateIsAtBottom],
  );

  const context = useMemo(
    () => ({ isAtBottom, scrollToBottom }),
    [isAtBottom, scrollToBottom],
  );

  return (
    <ConversationContext.Provider value={context}>
      <div
        ref={scrollRef}
        className={cn("relative flex-1 overflow-y-auto", className)}
        onScroll={handleScroll}
        role={role}
        {...props}
      >
        {children}
      </div>
    </ConversationContext.Provider>
  );
};

export type ConversationContentProps = ComponentProps<"div">;

export const ConversationContent = ({
  className,
  ...props
}: ConversationContentProps) => (
  <div className={cn("flex flex-col gap-8 p-4", className)} {...props} />
);

export type ConversationScrollButtonProps = ComponentProps<typeof Button>;

export const ConversationScrollButton = ({
  className,
  ...props
}: ConversationScrollButtonProps) => {
  const context = useContext(ConversationContext);
  if (!context || context.isAtBottom) return null;

  return (
    <Button
      aria-label="Scroll to latest message"
      className={cn(
        "absolute bottom-4 left-[50%] translate-x-[-50%] rounded-full dark:bg-background dark:hover:bg-muted",
        className,
      )}
      onClick={context.scrollToBottom}
      size="icon"
      type="button"
      variant="outline"
      {...props}
    >
      <ArrowDownIcon className="size-4" />
    </Button>
  );
};
