interface CueTiming {
  start_us: number;
  end_us: number;
}

/**
 * Keeps the selected cue array referentially stable between authored cue
 * boundaries. The media clock can still update on every presented frame, but
 * subtitle filtering and child DOM updates only run when visibility can change.
 */
export function createCueBoundaryCache<T, R>(
  timing: (item: T) => CueTiming,
  select: (items: T[], positionUs: number) => R,
): (items: T[], positionUs: number) => R {
  let previousItems: T[] | null = null;
  let validFromUs = Number.NEGATIVE_INFINITY;
  let validUntilUs = Number.POSITIVE_INFINITY;
  let previousResult: R;
  let initialized = false;

  return (items, positionUs) => {
    if (
      initialized &&
      items === previousItems &&
      positionUs >= validFromUs &&
      positionUs < validUntilUs
    ) {
      return previousResult;
    }

    validFromUs = Number.NEGATIVE_INFINITY;
    validUntilUs = Number.POSITIVE_INFINITY;
    for (const item of items) {
      const cue = timing(item);
      for (const boundary of [cue.start_us, cue.end_us]) {
        if (boundary <= positionUs) validFromUs = Math.max(validFromUs, boundary);
        else validUntilUs = Math.min(validUntilUs, boundary);
      }
    }

    previousItems = items;
    previousResult = select(items, positionUs);
    initialized = true;
    return previousResult;
  };
}
