import http from "node:http";
import https from "node:https";
import net from "node:net";
import fs from "node:fs";
import fsp from "node:fs/promises";
import path from "node:path";

function fail(message) {
  console.error(message);
  process.exit(1);
}

function parseArgs(argv) {
  const args = new Map();
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (!key?.startsWith("--") || value === undefined) {
      fail(
        "Usage: node webapp-server.mjs --port <port> --api-origin <origin> --root <dist>",
      );
    }
    args.set(key.slice(2), value);
  }
  return {
    port: Number.parseInt(args.get("port") ?? "", 10),
    apiOrigin: args.get("api-origin"),
    root: args.get("root"),
  };
}

const options = parseArgs(process.argv.slice(2));

if (!Number.isInteger(options.port) || options.port <= 0) {
  fail("Expected a valid --port value");
}

if (!options.apiOrigin) {
  fail("Expected --api-origin");
}

if (!options.root) {
  fail("Expected --root");
}

const apiOrigin = new URL(options.apiOrigin);
const uiRoot = path.resolve(options.root);

const mimeTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".ico", "image/x-icon"],
  [".jpeg", "image/jpeg"],
  [".jpg", "image/jpeg"],
  [".js", "application/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".map", "application/json; charset=utf-8"],
  [".png", "image/png"],
  [".svg", "image/svg+xml; charset=utf-8"],
  [".wasm", "application/wasm"],
  [".woff", "font/woff"],
  [".woff2", "font/woff2"],
]);

function isApiPath(pathname) {
  return (
    pathname === "/health" ||
    pathname === "/mcp" ||
    pathname.startsWith("/api/") ||
    pathname.startsWith("/mcp/")
  );
}

function proxyHttp(request, response) {
  const transport = apiOrigin.protocol === "https:" ? https : http;
  const proxyRequest = transport.request(
    {
      protocol: apiOrigin.protocol,
      hostname: apiOrigin.hostname,
      port: apiOrigin.port,
      method: request.method,
      path: request.url,
      headers: {
        ...request.headers,
        host: apiOrigin.host,
      },
    },
    (proxyResponse) => {
      response.writeHead(proxyResponse.statusCode ?? 502, proxyResponse.headers);
      proxyResponse.pipe(response);
    },
  );

  proxyRequest.on("error", (error) => {
    response.writeHead(502, { "content-type": "text/plain; charset=utf-8" });
    response.end(`Upstream request failed: ${error.message}`);
  });

  request.pipe(proxyRequest);
}

async function serveStatic(request, response) {
  const requestUrl = new URL(request.url ?? "/", "http://127.0.0.1");
  const pathname = decodeURIComponent(requestUrl.pathname);
  const relativePath = pathname === "/" ? "index.html" : pathname.slice(1);
  const candidatePath = path.resolve(uiRoot, relativePath);
  const insideRoot = candidatePath === uiRoot || candidatePath.startsWith(`${uiRoot}${path.sep}`);

  let filePath = path.join(uiRoot, "index.html");
  let stat = null;

  if (insideRoot) {
    try {
      stat = await fsp.stat(candidatePath);
      if (stat.isFile()) {
        filePath = candidatePath;
      }
    } catch {}
  }

  try {
    const stream = fs.createReadStream(filePath);
    const contentType =
      mimeTypes.get(path.extname(filePath).toLowerCase()) ??
      "application/octet-stream";
    response.writeHead(200, {
      "content-type": contentType,
      "cache-control": filePath.endsWith("index.html")
        ? "no-store"
        : "public, max-age=300",
    });
    stream.pipe(response);
  } catch (error) {
    response.writeHead(500, { "content-type": "text/plain; charset=utf-8" });
    response.end(`Failed to serve ${filePath}: ${error.message}`);
  }
}

function proxyUpgrade(request, clientSocket, head) {
  const upstreamSocket = net.connect(
    {
      host: apiOrigin.hostname,
      port:
        Number.parseInt(apiOrigin.port || "", 10) ||
        (apiOrigin.protocol === "https:" ? 443 : 80),
    },
    () => {
      const rawHeaders = Object.entries(request.headers)
        .map(([key, value]) => {
          const headerValue = Array.isArray(value) ? value.join(", ") : value ?? "";
          if (key.toLowerCase() === "host") {
            return `host: ${apiOrigin.host}`;
          }
          return `${key}: ${headerValue}`;
        })
        .join("\r\n");

      upstreamSocket.write(
        `GET ${request.url} HTTP/${request.httpVersion}\r\n${rawHeaders}\r\n\r\n`,
      );
      if (head.length > 0) {
        upstreamSocket.write(head);
      }

      clientSocket.pipe(upstreamSocket);
      upstreamSocket.pipe(clientSocket);
    },
  );

  const destroy = () => {
    if (!clientSocket.destroyed) {
      clientSocket.destroy();
    }
    if (!upstreamSocket.destroyed) {
      upstreamSocket.destroy();
    }
  };

  upstreamSocket.on("error", destroy);
  clientSocket.on("error", destroy);
}

const server = http.createServer(async (request, response) => {
  const requestUrl = new URL(request.url ?? "/", "http://127.0.0.1");
  if (isApiPath(requestUrl.pathname)) {
    proxyHttp(request, response);
    return;
  }

  await serveStatic(request, response);
});

server.on("upgrade", (request, socket, head) => {
  const requestUrl = new URL(request.url ?? "/", "http://127.0.0.1");
  if (!isApiPath(requestUrl.pathname)) {
    socket.destroy();
    return;
  }

  proxyUpgrade(request, socket, head);
});

server.listen(options.port, "127.0.0.1", () => {
  console.log(
    `Hypercolor e2e webapp listening on http://127.0.0.1:${options.port} -> ${apiOrigin.origin}`,
  );
});
