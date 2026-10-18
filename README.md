# NARO

Navegador desktop experimental escrito em Rust, com janela, interface e motor de
renderização próprios. Não usa WebView nem um motor pronto: HTML, CSS, layout e
pintura são implementados neste repositório, em um subconjunto pequeno e
deliberado da web real.

Esta é a **primeira entrega**: já existe um fluxo completo e utilizável —
`rede → HTML → CSS → layout → pintura` — com barra de endereços, voltar,
avançar, recarregar, histórico, rolagem e clique em links.

## Arquitetura

| Crate | Responsabilidade |
| --- | --- |
| `naro-app` | Janela (winit), chrome desenhado à mão, teclado/mouse, thread de rede, modos headless |
| `naro-net` | HTTP/HTTPS, resolução de URLs, normalização do que foi digitado |
| `naro-html` | Tokenizador e construtor de árvore DOM próprios |
| `naro-css` | Parser de CSS, seletores (combinadores, atributos, pseudo-classes), comprimentos (`rem`/`em`/`vw`/`calc()`…), cascata com `!important` e herança |
| `naro-layout` | Árvore de caixas, fluxo de blocos, quebra de linhas |
| `naro-paint` | Rasterização em software sobre um framebuffer 32-bit |
| `naro-text` | Carregamento de fonte do sistema, medição e rasterização de glifos |
| `naro-engine` | Pipeline, navegação, histórico, scroll, hit-test de links, dump |
| `naro-image` | Decodificação PNG/JPEG com limites de memória |
| `naro-js` | Runtime JavaScript (QuickJS via `rquickjs`) ligado ao DOM real |

Fluxo de dados:

```
fetch → Document::parse → compute_styles → layout → paint_page → softbuffer
```

Cada etapa é um crate separado justamente para poder ser substituída ou
aprofundada sem reescrever as outras.

## Requisitos

- Rust estável (testado com 1.99).
- Windows: o alvo `x86_64-pc-windows-gnu` funciona sem Visual Studio.
  ```bash
  rustup toolchain install stable-x86_64-pc-windows-gnu
  ```
  (O host MSVC também funciona, se houver Build Tools instalado.)
- Linux: `libssl-dev` e `pkg-config` para o `native-tls`, além de uma fonte
  DejaVu.

## Executar

```bash
cargo run --release --target x86_64-pc-windows-gnu -p naro-app
```

Ou, com MSVC:

```bash
cargo run --release -p naro-app
```

A janela abre em `naro://start`. Um endereço opcional abre direto:

```bash
cargo run --release --target x86_64-pc-windows-gnu -p naro-app -- example.com
```

### Verificação sem janela

```bash
# Árvore de layout e textos em texto puro
cargo run --release --target x86_64-pc-windows-gnu -p naro-app -- --dump naro://start

# Renderiza um frame em BMP (24-bit), sem abrir janela
cargo run --release --target x86_64-pc-windows-gnu -p naro-app -- --render naro://start out.bmp

# Um clique antes do frame, para inspecionar o estado depois da interação
cargo run --release --target x86_64-pc-windows-gnu -p naro-app -- --render pagina.html out.bmp 1000 20 12

# Estilo computado de cada nó que casa com um seletor (diagnóstico de layout)
cargo run --release --target x86_64-pc-windows-gnu -p naro-app -- --style https://example.com "body > p"

# Largura lógica customizada para dump/render (padrão 1000)
cargo run --release --target x86_64-pc-windows-gnu -p naro-app -- --dump naro://start 1200

# Uma URL real
cargo run --release --target x86_64-pc-windows-gnu -p naro-app -- --dump https://example.com

# Corpus de compatibilidade (tests/compat + manifest.txt)
cargo run --release --target x86_64-pc-windows-gnu -p naro-app -- --compat tests/compat
```

O `--dump` imprime, logo após `STATUS:`, uma linha `CSS:` com o que o parser da
folha de autor aceitou e descartou:

```
CSS: rules=N invalid=I pseudo-element=P unsupported-not=U at-rule=A interactive=V unsupported-pseudo=S
```

`invalid`, `pseudo-element`, `unsupported-not` e `at-rule` são regras descartadas
com motivo; `interactive` e `unsupported-pseudo` são pseudo-classes que parseiam mas
nunca casam na pintura estática.

Testes:

```bash
cargo test --workspace --target x86_64-pc-windows-gnu
```

## Atalhos

| Tecla | Ação |
| --- | --- |
| `Ctrl+L` | Foca a barra de endereços (texto selecionado) |
| `Enter` | Navega para o endereço digitado |
| `Esc` | Sai da barra de endereços |
| `Alt+←` / `Alt+→` | Voltar / avançar |
| `F5` ou `Ctrl+R` | Recarregar |
| Roda do mouse | Rolar a página |
| Clique | Seguir links, usar botões, focar a barra |

Endereços sem esquema recebem `https://`. Caminhos locais existentes viram
`file://`. `naro://start` é a página inicial.

## JavaScript

Cada documento recebe um `Runtime` QuickJS próprio (`rquickjs`). Os scripts são
executados após o parsing e antes da cascata de estilo, de modo que as mutações
aparecem na primeira pintura. Uma navegação descarta o runtime inteiro, então
referências antigas não alcançam o documento novo.

User-Agent: as requisições se identificam como um Chrome com o sufixo `NARO/0.1`.
Alguns sites (o Google entre eles) respondem a um agente não-navegador com uma
resposta 200 **vazia** para as folhas de estilo, o que descarta a cascata inteira.

Implementado: `document.getElementById`, `querySelector`/`querySelectorAll` (no
documento e em elementos, com a mesma gramática do CSS: combinadores, atributos,
`:not`, `:nth-child`, listas por vírgula), `createElement`, `createTextNode`,
`appendChild`/`insertBefore`/`removeChild`/`remove`, `append`/`prepend`,
`insertAdjacentHTML` (as quatro posições), `textContent`/`innerText`, `innerHTML`,
`getAttribute`, `setAttribute`, `removeAttribute`, `dataset`, `matches`, `closest`,
`className`, `classList`, `style`, `children`/`childNodes`/`parentNode`/
`parentElement`/`firstChild`/`lastChild`/`nextSibling`/`previousSibling`/`nodeType`/
`isConnected`, `getBoundingClientRect`, `offsetWidth`/`offsetHeight`/`offsetLeft`/
`offsetTop`, `clientWidth`/`clientHeight`, `document.title`, `location`,
`navigator.language`, `console.log/error`, `setTimeout`/`setInterval`,
`queueMicrotask`, `MutationObserver` (`attributes`, `childList`, `subtree`,
`attributeFilter`), `addEventListener`, `window`.

Não implementado: `Image`, `fetch`/XHR, `localStorage`, `Canvas`, eventos sintéticos
reais, módulos ES.

### Identidade e medição

Um nó DOM tem **um** objeto JavaScript enquanto viver: `querySelector("#x")` duas
vezes devolve o mesmo objeto, e um objeto guardado continua descrevendo o mesmo nó
depois de removido (`parentNode` vira `null`, `isConnected` vira `false`). O cache é
por `NodeId` e guarda `WeakRef`, então não cresce numa página que troca nós
constantemente.

`getBoundingClientRect()` e as propriedades `offset*`/`client*` leem a geometria do
layout real — a mesma que o `--dump` imprime — e são atualizadas a cada relayout,
inclusive antes dos timers que rodam logo depois de `DOMContentLoaded`. Limitações:
`offsetLeft`/`offsetTop` são medidos contra o **pai direto** (não contra o ancestral
posicionado mais próximo, como no navegador), `client*` não desconta barras de
rolagem (não existem) e elementos sem caixa (ex.: `display:none`) medem zero.

### Atualização dinâmica

As mutações feitas por JavaScript não são aplicadas no ato. Cada uma registra o que
ficou desatualizado — estilo, layout ou só pintura — e a engine consolida tudo uma vez
por tarefa, na ordem estilo → layout → pintura:

- atributo, `classList`, `style`, `<style>` inserido/alterado/removido e nós
  inseridos/removidos marcam **estilo**;
- o estilo recalculado é comparado com o anterior: se a geometria pode mudar
  (`width`, `font-size`, `display`, margens, padding, grid…) o **layout** também é
  refeito; se só cor/fundo/borda mudaram, os dados de pintura das caixas existentes
  são atualizados e o layout é preservado;
- digitar num campo marca apenas **pintura**.

Uma tarefa com várias mutações custa uma única recascata. `MutationObserver` continua
notificando em microtask, independente da invalidação: uma página sem observer é
atualizada do mesmo jeito. O `--dump` mostra o resultado em
`SYNC: restyle=N relayout=N repaint=N`.

## Limitações explícitas

Nada abaixo é suportado — e a interface não finge que é:

- JavaScript: subconjunto mínimo (ver abaixo), longe do suficiente para aplicações modernas.
- Sem CSS externo (`<link rel=stylesheet>`) nem `@import`; só `<style>` e o
  atributo `style`.
- Imagens PNG/JPEG e `background-image` (esticado). `<svg>` inline é desenhado
  (paths preenchidos com a regra nonzero, `fill`/`currentColor`); sem strokes,
  gradientes, `transform` ou `clip-path`. Sem GIF, `srcset`, iframes ou fontes web.
- Formulários: `input`, `textarea` e `button` com foco, digitação e eventos
  `focus`/`blur`/`input`/`change`/`click`. Sem checkbox, radio ou `select`.
- Seletores: combinadores ` `, `>`, `+`, `~`; atributos (`[a]`, `=`, `~=`, `^=`,
  `$=`, `*=`, `|=`); `:root`, `:first-child`, `:last-child`, `:nth-child()`,
  `:not()` e `!important`. Pseudo-elementos são descartados (contados no relatório),
  pseudo-classes de interação (`:hover`, `:focus`, `:active`…) parseiam mas nunca
  casam na pintura estática.
- Comprimentos: `px`, `em`, `rem`, `%`, `vw`, `vh`, `vmin`, `vmax` e as funções
  `calc()`, `min()`, `max()`, `clamp()`. `em` em `font-size` usa o tamanho do pai;
  nas demais propriedades usa o do próprio elemento. Percentuais de
  `margin`/`padding` referenciam a **largura** do bloco contentor (inclusive nos
  lados verticais); percentual de `height` contra um contentor de altura automática
  resolve como `auto`, porque a altura do pai vem do conteúdo. Sem `pt`, `pc`,
  `ch`, `ex`, `dvh`/`svh`/`lvh` nem `border-radius` percentual.
- Layout: block, inline, `inline-block`, `display: contents`, flex (row/column,
  wrap, `flex-grow`/`shrink`/`basis` e o atalho `flex`, `justify-content`,
  `align-items`/`align-self`/`align-content`, `gap`, margens automáticas), grid
  (`fr`/`%`/`minmax()`/`repeat(auto-fit|auto-fill)`, spans, `gap`, alinhamento
  vertical dos itens) e tabelas (`colspan`/`rowspan`, colunas compartilhadas entre
  linhas, `table-layout: fixed`). `position: relative`, `absolute` e `fixed`,
  `overflow` com recorte, `min-height`/`max-height`. Sem floats, `grid-area`,
  linhas nomeadas, `order`, `border-collapse` nem `vertical-align`.
- Sem seleção ou cópia de texto, abas, downloads, cookies, cache HTTP ou
  validação de certificados customizada.
- Texto é UTF-8 lossy; sem shaping, kerning ou ligaduras.
- Pintura na CPU (softbuffer), sem GPU.
- Sem sandbox de processo: a rede roda em uma thread, o conteúdo não é isolado.
- Um único documento por vez, sem histórico de formulários.

## Roadmap

1. CSS externo e imagens.
2. Seleção e cópia de texto.
3. Cache HTTP e cabeçalhos condicionais.
4. Compositor em GPU preservando a interface de `naro-paint`.
5. Isolamento de processos para conteúdo remoto.
6. Motor JavaScript em crate próprio.

## Licença

MIT — veja `LICENSE`.
