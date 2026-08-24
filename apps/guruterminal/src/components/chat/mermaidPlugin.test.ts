import { hasCompleteMermaidFence } from "./mermaidPlugin";

describe("hasCompleteMermaidFence", () => {
  it.each([
    ["ordinary Markdown", "A normal response with no diagram.", false],
    ["another fenced language", "```text\nmermaid\n```", false],
    ["an unclosed Mermaid fence", "```mermaid\ngraph TD\n  A --> B", false],
    ["a shorter closing fence", "````mermaid\ngraph TD\n```", false],
    ["a complete backtick Mermaid fence", "```mermaid\ngraph TD\n  A --> B\n```", true],
    ["a complete tilde Mermaid fence", "~~~MERMAID\ngraph TD\n  A --> B\n~~~", true],
  ])("recognizes %s", (_caseName, markdown, expected) => {
    expect(hasCompleteMermaidFence(markdown)).toBe(expected);
  });
});
