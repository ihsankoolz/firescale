/**
 * An error whose message is safe to show to a user as is. It names the
 * problem and the next step to take.
 */
export class FirescaleError extends Error {
  override readonly name = 'FirescaleError';
}
