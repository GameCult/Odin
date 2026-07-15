"use strict";

function createProviderSessionIngress(options) {
  const { CultMesh, CultMeshProviderSessionBroker, decodeProviderConnectEvidence } = options.runtime;
  const broker = new CultMeshProviderSessionBroker({
    runtimeId: options.runtimeId,
    expiryPollMs: options.expiryPollMs,
    authorizeRegistration: (_identity, session) => {
      if (!options.sessionToken) return false;
      try {
        return decodeProviderConnectEvidence(session.connectPayload).sessionToken === options.sessionToken;
      } catch {
        return false;
      }
    },
    acceptPublication: (identity, _publicationId, document) => options.source.accept(identity, document),
    deletePublications: (identity, publications) => {
      for (const publication of publications) options.source.withdraw(identity, publication.document, "provider-session-ended");
    },
    acceptReceipt: options.acceptReceipt ?? (() => {}),
    onError: options.onError,
  });
  const server = CultMesh.createRudpDocumentServer(options.runtimeId, options.connectionId ?? 0x43554c54, {
    bindHost: options.bindHost,
    bindPort: options.bindPort,
    documents: new options.CultNetDocumentRegistry(),
    onOperationRequest: (request, session) => broker.handle(request, session),
    onSessionClosed: session => broker.sessionClosed(session),
    onError: options.onError,
  });
  return {
    get bind() { return server.bind; },
    start: () => server.start(),
    close: () => { broker.close(); server.close(); },
  };
}

module.exports = { createProviderSessionIngress };
