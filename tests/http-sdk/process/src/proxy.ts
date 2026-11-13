/** ADR-0046 AU-23：真实 HTTP/WebSocket 反向代理，监听与连接随测试进程退出。 */
import {createServer, ServerResponse} from "node:http";
import {writeFileSync} from "node:fs";
import type {AddressInfo, Socket} from "node:net";
import httpProxy from "http-proxy";

const proxy = httpProxy.createProxyServer({target: process.argv[2], ws: true, xfwd: true});
const sockets = new Set<Socket>();
const server = createServer((request, response) => proxy.web(request, response));
proxy.on("error", (_error, _request, response) => {
    if (response instanceof ServerResponse && !response.headersSent) {
        response.writeHead(502, {"Content-Type": "text/plain"});
        response.end("Bad Gateway");
    } else response.destroy();
});
server.on("upgrade", (request, socket, head) => proxy.ws(request, socket, head));
server.on("connection", socket => { sockets.add(socket); socket.once("close", () => sockets.delete(socket)); });
server.listen(0, "127.0.0.1", () => {
    const address = server.address() as AddressInfo;
    writeFileSync(process.argv[3], JSON.stringify({port: address.port, pid: process.pid}));
});
process.once("SIGTERM", () => {
    for (const socket of sockets) socket.destroy();
    proxy.close();
    server.close(() => process.exit(0));
});
