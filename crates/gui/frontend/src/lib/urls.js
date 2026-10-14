export function siteUrl(domain, { https = true, httpPort = 80, httpsPort = 443 } = {}) {
  if (https) {
    return httpsPort === 443 ? `https://${domain}` : `https://${domain}:${httpsPort}`;
  }
  return httpPort === 80 ? `http://${domain}` : `http://${domain}:${httpPort}`;
}
