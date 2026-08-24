import { cn } from "@/lib/utils";
import { parseCredentialFreeHttpUrl } from "@/lib/credentialFreeUrl";
import { cjk } from "@streamdown/cjk";
import { code } from "@streamdown/code";
import { math } from "@streamdown/math";
import { mermaid } from "@streamdown/mermaid";
import { ExternalLinkIcon } from "lucide-react";
import type { ComponentProps } from "react";
import { memo, useMemo, useState } from "react";
import { Streamdown } from "streamdown";

type MarkdownLinkProps = ComponentProps<"a"> & { node?: unknown };

function GuruTerminalMarkdownLink({
  children,
  className,
  href,
  node: _node,
  onOpenLink,
  rel: _rel,
  target: _target,
  ...anchorProps
}: MarkdownLinkProps & {
  onOpenLink: (url: string) => Promise<void> | void;
}) {
  const [openFailed, setOpenFailed] = useState(false);
  const externalUrl =
    href && /^https?:\/\//i.test(href)
      ? (parseCredentialFreeHttpUrl(href)?.href ?? null)
      : null;

  if (href?.startsWith("#")) {
    return (
      <a
        {...anchorProps}
        className={className}
        data-streamdown="link"
        href={href}
      >
        {children}
      </a>
    );
  }

  if (!externalUrl) {
    return (
      <span
        className={cn("guru-blocked-link", className)}
        data-streamdown="link"
        title="This link type cannot be opened"
      >
        {children}
        <span className="sr-only">Unsupported link.</span>
      </span>
    );
  }

  return (
    <button
      type="button"
      role="link"
      className={cn("guru-external-link", className)}
      data-streamdown="link"
      title={openFailed ? "Could not open this link" : `Open ${externalUrl}`}
      data-open-failed={openFailed || undefined}
      onClick={() => {
        setOpenFailed(false);
        void Promise.resolve()
          .then(() => onOpenLink(externalUrl))
          .catch(() => {
            setOpenFailed(true);
          });
      }}
    >
      {children}
      <ExternalLinkIcon aria-hidden="true" />
      {openFailed && (
        <span className="sr-only" role="alert">
          Could not open this link.
        </span>
      )}
    </button>
  );
}

export type MessageResponseProps = ComponentProps<typeof Streamdown> & {
  onOpenLink?: (url: string) => Promise<void> | void;
};

const streamdownPlugins = { cjk, code, math, mermaid };
const disabledLinkSafety = { enabled: false } as const;

export const MessageResponse = memo(
  ({
    className,
    components,
    children,
    linkSafety,
    onOpenLink,
    ...props
  }: MessageResponseProps) => {
    const resolvedComponents = useMemo(
      () =>
        onOpenLink
          ? {
              ...components,
              a: (linkProps: MarkdownLinkProps) => (
                <GuruTerminalMarkdownLink
                  {...linkProps}
                  onOpenLink={onOpenLink}
                />
              ),
            }
          : components,
      [components, onOpenLink],
    );

    return (
      <Streamdown
        className={cn(
          "size-full [&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
          className,
        )}
        components={resolvedComponents}
        linkSafety={onOpenLink ? disabledLinkSafety : linkSafety}
        plugins={streamdownPlugins}
        {...props}
      >
        {children}
      </Streamdown>
    );
  },
);

MessageResponse.displayName = "MessageResponse";
