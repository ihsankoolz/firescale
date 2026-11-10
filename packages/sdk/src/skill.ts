/**
 * Validation for a rule's SKILL.md. The layout is fixed by SPEC §5 so that
 * humans and agents always find the same sections in the same order.
 */

/** Frontmatter fields defined by the Agent Skills spec (agentskills.io). No others are allowed. */
export const SKILL_FRONTMATTER_FIELDS: readonly string[] = [
  'name',
  'description',
  'license',
  'compatibility',
  'metadata',
  'allowed-tools',
];

/** The `##` sections every SKILL.md must have, in this order. */
export const SKILL_SECTIONS: readonly string[] = [
  'What this rule enforces and why',
  'Manual fix',
  'Agent fix instructions',
  'Verify',
];

/** The `###` blocks required inside "Agent fix instructions", in this order. */
export const SKILL_AGENT_BLOCKS: readonly string[] = ['DETERMINISTIC', 'REPORT'];

export interface ParsedSkill {
  /** Top-level frontmatter fields with single-line values. */
  frontmatter: Record<string, string>;
  /** Everything after the frontmatter, with LF line endings. */
  body: string;
}

export interface SkillExpectation {
  /** Full rule id, `<pack>/<rule>`. When set, name and verify command must match it. */
  ruleId?: string;
}

function unquote(value: string): string {
  const quoted = /^(["'])(.*)\1$/.exec(value);
  return quoted?.[2] ?? value;
}

/** Splits a SKILL.md into frontmatter and body. Returns null when there is no frontmatter. */
export function parseSkill(text: string): ParsedSkill | null {
  const normalized = text.replace(/^\uFEFF/, '').replaceAll('\r\n', '\n');
  const match = /^---\n([\s\S]*?)\n---\n?([\s\S]*)$/.exec(normalized);
  if (!match) return null;
  const frontmatter: Record<string, string> = {};
  for (const line of (match[1] ?? '').split('\n')) {
    const field = /^([A-Za-z][\w-]*):\s*(.*)$/.exec(line);
    if (field?.[1] !== undefined) frontmatter[field[1]] = unquote((field[2] ?? '').trim());
  }
  return { frontmatter, body: match[2] ?? '' };
}

function sectionBody(body: string, heading: string, level: number): string | null {
  const lines = body.split('\n');
  const marker = `${'#'.repeat(level)} ${heading}`;
  const start = lines.findIndex((line) => line.trim() === marker);
  if (start === -1) return null;
  const stop = new RegExp(`^#{1,${level}} `);
  const rest = lines.slice(start + 1);
  const end = rest.findIndex((line) => stop.test(line));
  return (end === -1 ? rest : rest.slice(0, end)).join('\n');
}

/** Returns a list of problems. An empty list means the SKILL.md is valid. */
export function validateSkill(text: string, expected: SkillExpectation = {}): string[] {
  const parsed = parseSkill(text);
  if (!parsed) {
    return [
      'Missing frontmatter. Start the file with a `---` block that has name and description.',
    ];
  }
  const problems: string[] = [];
  const { frontmatter, body } = parsed;

  for (const field of Object.keys(frontmatter)) {
    if (!SKILL_FRONTMATTER_FIELDS.includes(field)) {
      problems.push(
        `Frontmatter field "${field}" is not part of the Agent Skills spec. Remove it.`,
      );
    }
  }

  const name = frontmatter['name'];
  if (!name) {
    problems.push('Frontmatter is missing "name".');
  } else {
    if (name.length > 64 || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)) {
      problems.push(
        `Frontmatter name "${name}" must be at most 64 lowercase letters, digits and single hyphens.`,
      );
    }
    const expectedName = expected.ruleId?.split('/')[1];
    if (expectedName !== undefined && name !== expectedName) {
      problems.push(`Frontmatter name "${name}" must be "${expectedName}" to match the rule id.`);
    }
  }

  const description = frontmatter['description'];
  if (!description) {
    problems.push('Frontmatter is missing "description".');
  } else if (description.length > 1024) {
    problems.push('Frontmatter description must be at most 1024 characters.');
  }

  let cursor = -1;
  for (const section of SKILL_SECTIONS) {
    const index = body.split('\n').findIndex((line) => line.trim() === `## ${section}`);
    if (index === -1) {
      problems.push(`Missing section "## ${section}".`);
    } else if (index < cursor) {
      problems.push(
        `Section "## ${section}" is out of order. Expected: ${SKILL_SECTIONS.join(', ')}.`,
      );
    } else {
      cursor = index;
      if ((sectionBody(body, section, 2) ?? '').trim() === '') {
        problems.push(`Section "## ${section}" is empty.`);
      }
    }
  }

  const agent = sectionBody(body, 'Agent fix instructions', 2);
  if (agent !== null) {
    let blockCursor = -1;
    for (const block of SKILL_AGENT_BLOCKS) {
      const index = agent.split('\n').findIndex((line) => line.trim() === `### ${block}`);
      if (index === -1) {
        problems.push(`"Agent fix instructions" is missing the "### ${block}" block.`);
      } else if (index < blockCursor) {
        problems.push(`"### ${block}" is out of order. Put DETERMINISTIC before REPORT.`);
      } else {
        blockCursor = index;
        if ((sectionBody(agent, block, 3) ?? '').trim() === '') {
          problems.push(`"### ${block}" is empty. Write "None." if there is nothing to list.`);
        }
      }
    }
  }

  const verify = sectionBody(body, 'Verify', 2);
  if (verify !== null) {
    const command = expected.ruleId
      ? `fscale check --rules ${expected.ruleId}`
      : 'fscale check --rules ';
    if (!verify.includes(command)) {
      problems.push(
        `"Verify" must contain the exact command \`fscale check --rules ${expected.ruleId ?? '<pack>/<rule>'}\`.`,
      );
    }
  }

  return problems;
}
