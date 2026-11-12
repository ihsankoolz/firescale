// The generator interface (SPEC §16, ADR 0028). A generator turns JSON input,
// checked against its JSON Schema first, into new files. It never edits a
// file that exists: fscale refuses the run before anything is written.

/** The subset of JSON Schema that generator inputs use. */
export interface InputSchema {
  $schema: 'http://json-schema.org/draft-07/schema#';
  title: string;
  description: string;
  type: 'object';
  additionalProperties: false;
  required: string[];
  properties: Record<string, PropertySchema>;
}

export interface PropertySchema {
  type: 'string' | 'boolean';
  description: string;
  pattern?: string;
  enum?: string[];
  default?: string | boolean;
  maxLength?: number;
}

/** A file a generator creates, by repo-relative path with forward slashes. */
export interface GeneratedFile {
  path: string;
  content: string;
}

export interface GeneratorOutput {
  files: GeneratedFile[];
  /** Things to tell the person afterwards, such as a list to add the new package to. */
  notes: string[];
}

/** What a generator may read about the repo. */
export interface GeneratorContext {
  /** Absolute path of the repo root. */
  root: string;
  /** True when a repo-relative path exists. */
  exists(file: string): boolean;
  /** A repo-relative text file, or null. */
  read(file: string): string | null;
}

export interface Generator {
  name: string;
  description: string;
  schema: InputSchema;
  /** Input has been validated against `schema`, with defaults filled in. */
  generate(input: Record<string, string | boolean>, ctx: GeneratorContext): GeneratorOutput;
}
