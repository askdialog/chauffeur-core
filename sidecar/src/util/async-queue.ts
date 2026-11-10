/**
 * An unbounded single-consumer queue readable as an AsyncIterable: `push` never blocks, the
 * consumer waits when the queue is empty, and `end` finishes the iteration once drained.
 */
export class AsyncQueue<T> implements AsyncIterable<T> {
  private readonly items: T[] = [];
  private waiter: ((result: IteratorResult<T>) => void) | undefined;
  private ended = false;

  get isEnded(): boolean {
    return this.ended;
  }

  /** Items pushed and not read yet. */
  get size(): number {
    return this.items.length;
  }

  /** Returns false when the queue has already ended (the item is dropped). */
  push(item: T): boolean {
    if (this.ended) return false;
    const waiter = this.waiter;
    if (waiter) {
      this.waiter = undefined;
      waiter({ value: item, done: false });
    } else {
      this.items.push(item);
    }
    return true;
  }

  end(): void {
    if (this.ended) return;
    this.ended = true;
    const waiter = this.waiter;
    this.waiter = undefined;
    waiter?.({ value: undefined, done: true });
  }

  [Symbol.asyncIterator](): AsyncIterator<T> {
    return {
      next: () => {
        if (this.items.length > 0) return Promise.resolve({ value: this.items.shift() as T, done: false });
        if (this.ended) return Promise.resolve({ value: undefined, done: true });
        return new Promise((resolve) => {
          this.waiter = resolve;
        });
      },
      return: () => {
        this.end();
        return Promise.resolve({ value: undefined, done: true });
      },
    };
  }
}
