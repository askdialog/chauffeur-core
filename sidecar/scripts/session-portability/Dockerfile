FROM oven/bun:1.4.3
WORKDIR /proof
ENV PATH="/proof/node_modules/.bin:${PATH}"
COPY package.json ./
RUN bun install && ln -s /usr/local/bin/bun /usr/local/bin/node
COPY phase.js ./
ENTRYPOINT ["bun", "/proof/phase.js"]
