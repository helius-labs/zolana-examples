/** Little-endian Borsh-style byte writer. */
export class ByteWriter {
  private readonly chunks: Uint8Array[] = [];

  bytes(value: ArrayLike<number>): this {
    this.chunks.push(Uint8Array.from(value));
    return this;
  }

  u8(value: number): this {
    if (!Number.isInteger(value) || value < 0 || value > 0xff) {
      throw new RangeError(`u8 out of range: ${value}`);
    }
    return this.bytes(Uint8Array.of(value));
  }

  u16(value: number): this {
    if (!Number.isInteger(value) || value < 0 || value > 0xffff) {
      throw new RangeError(`u16 out of range: ${value}`);
    }
    const out = new Uint8Array(2);
    new DataView(out.buffer).setUint16(0, value, true);
    return this.bytes(out);
  }

  u32(value: number): this {
    if (!Number.isInteger(value) || value < 0 || value > 0xffff_ffff) {
      throw new RangeError(`u32 out of range: ${value}`);
    }
    const out = new Uint8Array(4);
    new DataView(out.buffer).setUint32(0, value, true);
    return this.bytes(out);
  }

  u64(value: bigint): this {
    if (value < 0n || value >= 1n << 64n) {
      throw new RangeError(`u64 out of range: ${value}`);
    }
    const out = new Uint8Array(8);
    new DataView(out.buffer).setBigUint64(0, value, true);
    return this.bytes(out);
  }

  u128(value: bigint): this {
    if (value < 0n || value >= 1n << 128n) {
      throw new RangeError(`u128 out of range: ${value}`);
    }
    const out = new Uint8Array(16);
    const view = new DataView(out.buffer);
    view.setBigUint64(0, value & 0xffff_ffff_ffff_ffffn, true);
    view.setBigUint64(8, value >> 64n, true);
    return this.bytes(out);
  }

  finish(): Uint8Array {
    const length = this.chunks.reduce((sum, chunk) => sum + chunk.length, 0);
    const out = new Uint8Array(length);
    let offset = 0;
    for (const chunk of this.chunks) {
      out.set(chunk, offset);
      offset += chunk.length;
    }
    return out;
  }
}

/** Little-endian Borsh-style byte reader that fails on truncated input. */
export class ByteReader {
  private offset = 0;

  constructor(
    private readonly data: Uint8Array,
    private readonly label: string,
  ) {}

  bytes(length: number, field: string): Uint8Array {
    const end = this.offset + length;
    if (end > this.data.length) {
      throw new Error(`${this.label} truncated at ${field}`);
    }
    const out = this.data.slice(this.offset, end);
    this.offset = end;
    return out;
  }

  u8(field: string): number {
    return this.view(1, field).getUint8(0);
  }

  u16(field: string): number {
    return this.view(2, field).getUint16(0, true);
  }

  u32(field: string): number {
    return this.view(4, field).getUint32(0, true);
  }

  u64(field: string): bigint {
    return this.view(8, field).getBigUint64(0, true);
  }

  u128(field: string): bigint {
    const view = this.view(16, field);
    return view.getBigUint64(0, true) | (view.getBigUint64(8, true) << 64n);
  }

  /** Fails unless every byte was read. */
  end(): void {
    if (this.offset !== this.data.length) {
      throw new Error(
        `${this.label} has ${this.data.length - this.offset} trailing bytes`,
      );
    }
  }

  private view(length: number, field: string): DataView {
    const bytes = this.bytes(length, field);
    return new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
}

export function bytesEqual(
  a: ArrayLike<number>,
  b: ArrayLike<number>,
): boolean {
  if (a.length !== b.length) return false;
  for (let index = 0; index < a.length; index += 1) {
    if (a[index] !== b[index]) return false;
  }
  return true;
}
