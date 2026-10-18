# Sprint 21 — Media Queries e MutationObserver

Captura: `reports/google_s21.png` (comparar com `google_s20.png`).

## Causas confirmadas

| # | Evidência | Causa |
|---|---|---|
| 1 | `@media` era descartado inteiro por `strip_at_rules` | Nenhuma regra responsiva chegava à cascata; o campo do Google perdia raio, sombra e fundo |
| 2 | `MutationObserver` não existia | As classes aplicadas por JS após `DOMContentLoaded` nunca chegavam ao layout |
| 3 | Wrapper `<div style="position:relative;zoom:1">` não virava bloco contentor | Só elementos `display:block` podiam ser bloco contentor; um `<div>` inline posicionado era achatado no fluxo e as imagens absolutas dentro dele resolviam contra a raiz |

## Implementação

| Frente | Arquivo | Detalhe |
|---|---|---|
| **`@media`** | `naro-css` | `media_matches()` avalia `screen`/`all`/`print` e `min-width`/`max-width` em px contra a largura da viewport; blocos que casam têm o conteúdo recursivamente reprocessado, os demais são descartados. `parse_stylesheet_for(src, width)` nova; `parse_stylesheet` mantém 1000 px como padrão |
| **MutationObserver** | `naro-js` | `MutationObserver(callback)`, `observe(target, {attributes, childList, subtree, attributeFilter})`, `disconnect()`, `takeRecords()`; registros `{type, target, attributeName, addedNodes, removedNodes}`; entrega agrupada em microtask, sem reentrância |
| **Ancestral posicionado** | `naro-layout` | Elementos com `position` diferente de `static` são dispostos como blocos, tornando-se bloco contentor para descendentes absolutos |
| **Cascata** | `naro-engine` | `parse_stylesheet_for(…, self.viewport_width)` nos três pontos que recalculam estilo |

## Testes

`cargo test --workspace`: **62 passando, 0 falhas** (2 novos em `naro-css`):

- `media_queries_gate_rules_by_viewport` — `min-width:600` aplica em 800, `max-width:400` aplica em 300, `print` nunca aplica, CSS fora de `@media` preservado.
- `unsupported_media_features_do_not_leak` — `prefers-color-scheme` não vaza para o layout.

`naro --compat tests/compat`: **20 PASS, 0 FAIL, 1 UNSUP (total 21)**.
Novo caso `21-media-mutation.html` verifica, na mesma página:
`@media (min-width:600px)` aplicado (`color=#0a7d3f` no dump), `@media print` ignorado,
`MutationObserver` recebendo `attributes:data-x`, filtro `attributeFilter` descartando
`data-ignored`, e `takeRecords()` retornando 0 após a entrega.

## Comparação visual

| Aspecto | `google_s20.png` | `google_s21.png` |
|---|---|---|
| Erros JS | 0 | 0 |
| Campo de pesquisa | retângulo com borda | **idêntico** |
| Ícone do teclado | x=968, fora do campo | **idêntico** |
| Logo, botões, topo, rodapé | corretos | idênticos |

**O Google não mudou visualmente.** As duas funcionalidades estão implementadas e
testadas, mas não produziram efeito nesta página.

## Por que o campo continua sem estilo

Verificado no HTML servido:

```html
<div class="ds" style="height:32px;margin:4px 0">
  <div style="position:relative;zoom:1">
    <input class="lst Ucigb" style="margin:0;padding:5px 8px 0 6px;…;padding-right:38px">
    <img src="/textinputassistant/tia.png" style="position:absolute;right:5px;top:4px">
```

O wrapper tem `position:relative` **inline** e deveria ser o bloco contentor. Após a
mudança, o input ainda aparece em x=271 e a imagem em x=968 — ou seja, o wrapper
continua não formando bloco contentor. A causa concreta não foi isolada dentro do
tempo: o wrapper está dentro de `<table><tr><td>`, e **tabelas não são suportadas**
pelo layout; o `<td>` não gera caixa, então a hierarquia usada para o bloco contentor
não corresponde à do navegador.

O CSS do Google para `border-radius`/`box-shadow` do campo também não aparece nas
folhas servidas para esta variante da home (apenas `.lst{height:25px;width:496px}`),
então mesmo com `@media` funcionando não há regra a aplicar.

## Limitações

- Tabelas (`table`/`tr`/`td`) não geram caixas — bloqueia o bloco contentor do Google.
- `@media` não avalia `orientation`, `prefers-*`, `aspect-ratio`, `hover`.
- `MutationObserver` não cobre mutações feitas fora dos wrappers (ex.: `innerHTML` reporta só `childList` no alvo).
- `position: sticky` e `display: contents` continuam ausentes.

## Disco

12 GB livres antes e depois. Dois builds de release; nenhum diretório removido.
