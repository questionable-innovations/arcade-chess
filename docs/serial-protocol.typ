= Connection Protocol

ESP broadcasts:

```c
// Game state update
struct state {
  // valid moves for currently selected piece
  uint8_t valid_moves[8];
  
  // board eval 0-255
  uint8_t eval; 
  
  // first state object
  // state_a[0] -> picked up piece? (show valid moveset)
  // state_a[1] -> enable show valid moveset
  // state_a[2] -> in_game?
  // state_a[3] -> turn?
  // state_a[4] -> mode[0]
  // state_a[5] -> mode[1]
  // state_a[6] -> mode[2]
  // state_a[7] -> mode[3]
  uint8_t state_a;

  // second state object
  // state_b[0] -> relay[0]
  // state_b[1] -> relay[1]
  // state_b[2] -> relay[2]
  // state_b[3] -> relay[3]
  // state_b[4] -> show eval?
  // state_b[5] ->
  // state_b[6] ->
  // state_b[7] ->
  uint8_t state_b;
}



```