export function secondsToMicroseconds(seconds: number): number {
  if (!Number.isFinite(seconds) || seconds <= 0) return 0;
  return Math.round(seconds * 1_000_000);
}

export function microsecondsToSeconds(microseconds: number): number {
  if (!Number.isFinite(microseconds) || microseconds <= 0) return 0;
  return microseconds / 1_000_000;
}

export function formatTime(microseconds: number): string {
  const seconds = Math.max(0, Math.floor(microseconds / 1_000_000));
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const remainder = seconds % 60;
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, '0')}:${String(remainder).padStart(2, '0')}`
    : `${minutes}:${String(remainder).padStart(2, '0')}`;
}

export function isCueActive(startUs: number, endUs: number, positionUs: number): boolean {
  return startUs <= positionUs && positionUs < endUs;
}
