import { corePack } from '@firescale/rules-core';
import type { RulePack } from '@firescale/sdk';

/** Rule packs that ship with firescale, keyed by the name used in config. */
export const BUILTIN_PACKS: ReadonlyMap<string, RulePack> = new Map([[corePack.name, corePack]]);

/** Packs that are on when the config does not say otherwise. */
export const DEFAULT_PACKS: readonly string[] = [corePack.name];
