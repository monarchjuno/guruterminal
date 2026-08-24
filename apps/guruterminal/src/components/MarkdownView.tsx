import { useMemo, type ReactNode } from "react";

type FrontmatterField = {
  key: string;
  value: string;
};

type MarkdownHeading = {
  id: string;
  level: 1 | 2 | 3;
  text: string;
  lineIndex: number;
};

type ParsedMarkdown = {
  bodyLines: string[];
  frontmatter: FrontmatterField[];
  headings: MarkdownHeading[];
};

const inline = (text: string): ReactNode[] => {
  const parts = text.split(/(`[^`]+`|\*\*[^*]+\*\*)/g);
  return parts.filter(Boolean).map((part, index) => {
    if (part.startsWith("`") && part.endsWith("`")) {
      return <code key={`${part}-${index}`}>{part.slice(1, -1)}</code>;
    }
    if (part.startsWith("**") && part.endsWith("**")) {
      return <strong key={`${part}-${index}`}>{part.slice(2, -2)}</strong>;
    }
    return part;
  });
};

const headingText = (text: string) =>
  text.replace(/`([^`]+)`/g, "$1").replace(/\*\*([^*]+)\*\*/g, "$1").trim();

const slug = (text: string) =>
  text
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[^\p{Letter}\p{Number}]+/gu, "-")
    .replace(/^-+|-+$/g, "") || "section";

const parseFrontmatter = (lines: string[]) => {
  if (lines[0]?.trim() !== "---") {
    return { bodyLines: lines, frontmatter: [] as FrontmatterField[] };
  }

  const closingIndex = lines.findIndex(
    (line, index) => index > 0 && line.trim() === "---",
  );
  if (closingIndex === -1) {
    return { bodyLines: lines, frontmatter: [] as FrontmatterField[] };
  }

  const fields: FrontmatterField[] = [];
  for (const line of lines.slice(1, closingIndex)) {
    const match = /^([A-Za-z0-9_.-]+)\s*:\s*(.*)$/.exec(line);
    if (match) {
      fields.push({ key: match[1], value: match[2].trim() });
      continue;
    }
    const continuation = line.trim();
    if (continuation && fields.length) {
      const field = fields[fields.length - 1];
      field.value = [field.value, continuation].filter(Boolean).join("\n");
    }
  }

  return {
    bodyLines: lines.slice(closingIndex + 1),
    frontmatter: fields,
  };
};

const parseMarkdownDocument = (
  markdown: string,
  idPrefix = "document",
): ParsedMarkdown => {
  const normalized = markdown.replace(/\r\n?/g, "\n");
  const parsed = parseFrontmatter(normalized.split("\n"));
  const counts = new Map<string, number>();
  const prefix = `memory-${slug(idPrefix)}`;
  const headings: MarkdownHeading[] = [];

  parsed.bodyLines.forEach((raw, lineIndex) => {
    const match = /^(#{1,3})\s+(.+?)\s*$/.exec(raw.trim());
    if (!match) return;
    const text = headingText(match[2]);
    const base = `${prefix}-${slug(text)}`;
    const nextCount = (counts.get(base) ?? 0) + 1;
    counts.set(base, nextCount);
    headings.push({
      id: nextCount === 1 ? base : `${base}-${nextCount}`,
      level: match[1].length as 1 | 2 | 3,
      text,
      lineIndex,
    });
  });

  return { ...parsed, headings };
};

export function MarkdownView({
  markdown,
  idPrefix,
  showFrontmatter = false,
  showOutline = true,
}: {
  markdown: string;
  idPrefix?: string;
  showFrontmatter?: boolean;
  showOutline?: boolean;
}) {
  const document = useMemo(
    () => parseMarkdownDocument(markdown, idPrefix),
    [idPrefix, markdown],
  );
  const headingsByLine = useMemo(
    () => new Map(document.headings.map((heading) => [heading.lineIndex, heading])),
    [document.headings],
  );
  const nodes: ReactNode[] = [];
  let bullets: string[] = [];

  const flushBullets = () => {
    if (!bullets.length) return;
    const list = bullets;
    bullets = [];
    nodes.push(
      <ul key={`list-${nodes.length}`}>
        {list.map((item, index) => (
          <li key={`${item}-${index}`}>{inline(item)}</li>
        ))}
      </ul>,
    );
  };

  document.bodyLines.forEach((raw, index) => {
    const line = raw.trim();
    if (line.startsWith("- ")) {
      bullets.push(line.slice(2));
      return;
    }
    flushBullets();
    if (!line) return;

    const heading = headingsByLine.get(index);
    if (heading?.level === 3) {
      nodes.push(<h3 id={heading.id} key={index} tabIndex={-1}>{inline(line.slice(4))}</h3>);
    } else if (heading?.level === 2) {
      nodes.push(<h2 id={heading.id} key={index} tabIndex={-1}>{inline(line.slice(3))}</h2>);
    } else if (heading?.level === 1) {
      nodes.push(<h1 id={heading.id} key={index} tabIndex={-1}>{inline(line.slice(2))}</h1>);
    } else if (line.startsWith("> ")) {
      nodes.push(<blockquote key={index}>{inline(line.slice(2))}</blockquote>);
    } else if (line === "---") {
      nodes.push(<hr key={index} />);
    } else {
      nodes.push(<p key={index}>{inline(line)}</p>);
    }
  });
  flushBullets();

  return (
    <div className="markdown-document">
      {showFrontmatter && !!document.frontmatter.length && (
        <section className="markdown-frontmatter" aria-label="Document details">
          <div>
            <span>Details</span>
          </div>
          <dl>
            {document.frontmatter.map((field, index) => (
              <div key={`${field.key}-${index}`}>
                <dt>{field.key}</dt>
                <dd>{field.value || "—"}</dd>
              </div>
            ))}
          </dl>
        </section>
      )}

      {showOutline && !!document.headings.length && (
        <nav className="markdown-outline" aria-label="Document outline">
          <span>On this page</span>
          <div>
            {document.headings.map((heading) => (
              <a
                className={`level-${heading.level}`}
                href={`#${heading.id}`}
                key={heading.id}
              >
                {heading.text}
              </a>
            ))}
          </div>
        </nav>
      )}

      <article className="markdown-view">{nodes}</article>
    </div>
  );
}
