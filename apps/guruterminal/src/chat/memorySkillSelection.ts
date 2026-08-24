const WIKI_SKILL_ID = "wiki";
const LENS_SKILL_ID = "lens";

const MEMORY_SKILL_PATTERN = new RegExp(
  `(?:^|\\s)\\$(?:${WIKI_SKILL_ID}|${LENS_SKILL_ID})(?=\\s|$)`,
  "u",
);

export const promptSelectsMemorySkill = (prompt: string) =>
  MEMORY_SKILL_PATTERN.test(prompt);
