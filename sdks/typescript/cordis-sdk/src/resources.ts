/** ADR-0046 §3.8：本地资源按实际出口份额关闭，多个转交共享同一生命周期。 */
export abstract class OwnedResource {
    private exports = 0;
    retain(): void { this.exports++; }
    release(): void {
        if (--this.exports === 0) this.close();
    }
    abstract close(): void;
}
