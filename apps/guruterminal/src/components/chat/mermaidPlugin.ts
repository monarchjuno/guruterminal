import type { DiagramPlugin } from "@streamdown/mermaid";

const openingMermaidFence =
  /^[\t ]*(`{3,}|~{3,})[\t ]*mermaid(?:[\t ].*)?$/iu;

let pluginPromise: Promise<DiagramPlugin> | undefined;

export function hasCompleteMermaidFence(markdown: string): boolean {
  let openingFence: string | undefined;

  for (const line of markdown.split(/\r?\n/u)) {
    const activeFence = openingFence;
    if (!activeFence) {
      const opening = openingMermaidFence.exec(line);
      if (opening) {
        openingFence = opening[1];
      }
      continue;
    }

    const fence = line.trim();
    if (
      fence.length >= activeFence.length &&
      [...fence].every((character) => character === activeFence[0])
    ) {
      return true;
    }
  }

  return false;
}

export function loadMermaidPlugin(): Promise<DiagramPlugin> {
  if (!pluginPromise) {
    pluginPromise = import("@streamdown/mermaid")
      .then(({ mermaid }) => mermaid)
      .catch((error: unknown) => {
        pluginPromise = undefined;
        throw error;
      });
  }
  return pluginPromise;
}
