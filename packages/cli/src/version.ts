import { coreVersion } from '@firescale/native';

/**
 * The firescale version. It is read from the Rust core through the native
 * binding, so a working `fscale --version` proves the bridge is intact.
 */
export function getVersion(): string {
  return coreVersion();
}
